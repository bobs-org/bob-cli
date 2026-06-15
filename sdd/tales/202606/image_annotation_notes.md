---
create_time: 2026-06-15 10:38:23
status: wip
prompt: sdd/prompts/202606/image_annotation_notes.md
---
# Image Annotation Notes Plan

## Context

`bob highlights scan` and `bob highlights sync` now support TextBundle image selections from the Highlights app. The
current implementation detects Markdown image links in sidecar text, copies the referenced asset into a per-reference
`*.assets/` directory, renders the image in a `[!quote] Image` callout, and can render a nested `[!note] Comment` when
the parser attaches comment text to the image annotation.

The gap is that Bryan's notes/comments left on image annotations are not showing up in the generated reference note.
That makes image annotations incomplete: the visual selection is present, but the reasoning or follow-up text attached
to the annotation is lost from the Obsidian reference-note representation.

Important constraints from the current code and docs:

- The generated Highlights region is still the only body region owned by the command.
- Image block IDs and asset filenames are content-addressed from the source PDF path plus image bytes. Editing
  image-note text must not churn those IDs.
- Existing text highlight/comment/task behavior should remain unchanged.
- No new CLI surface is needed; this is a parser/rendering correctness fix for existing `scan` and `sync`.
- Highlights' own docs describe annotation comments as regular text below the annotation, and recommend TextBundle when
  notes contain images.

## Working Diagnosis

The relevant code lives in `src/native/highlights_ref/mod.rs`:

- `parse_image_sidecar_annotations()` builds `SidecarAnnotationKind::Image` annotations and attaches non-image lines in
  the same sidecar chunk as `comment` / `task_source`.
- `markdown_images_in_line()` currently captures only the image target and alt text.
- `SidecarAnnotation.text` is set to the image alt text or target, but `render_annotation_block()` ignores
  `annotation.text` for image annotations and renders only the embed plus `annotation.comment`.
- Existing tests cover a synthetic TextBundle shape where `Comment:` appears as a separate paragraph after the image.
  That path already renders correctly, so the missing real-world case is likely one of the image-specific export shapes
  not covered by tests: note text stored in the Markdown image metadata (`alt`/optional title), or a closely adjacent
  note/comment shape emitted by Highlights for image annotations.

## Goal

Make every user-authored note/comment associated with a Highlights image annotation visible in the generated reference
note, using the same nested `[!note] Comment` affordance already used for text highlights and image comments.

The fix should preserve:

- image asset copy behavior and idempotency,
- image block ID stability across note/comment edits,
- annotation task extraction from `#task` lines in image notes,
- marker mirror skipping,
- standalone sticky-note behavior,
- dry-run behavior and scan summaries.

## Design

1. Treat image annotation text as a first-class comment source.
   - Keep the image embed as the primary content of the `[!quote] Image` block.
   - Render any user-authored image note as the nested `[!note] Comment` block.
   - Never render the fallback image target path as user note text.

2. Extend Markdown image parsing conservatively.
   - Keep recognizing the currently supported `![alt](assets/file.png)` shape.
   - Parse optional Markdown image title text, such as `![alt](assets/file.png "note text")`, instead of dropping it
     while extracting the target.
   - Normalize angle-bracket targets and title-bearing targets without changing supported extension checks.

3. Combine possible image-note sources predictably.
   - Explicit sidecar lines adjacent to the image remain the strongest source because that is the documented "regular
     text below annotation" shape.
   - Markdown title text should be treated as user-authored note text.
   - Alt text should be considered only when it is non-empty and not duplicative of explicit comment/title text. If it
     appears to be the only note-bearing field in real fixtures, render it as the comment; otherwise keep it as metadata
     only to avoid noisy captions.
   - Preserve `task_source` from the same combined source so `#task` lines in image notes continue into the existing
     annotation-task pipeline.

4. Be careful about separate standalone notes.
   - Do not broadly merge arbitrary following standalone notes into images; that would risk stealing real sticky notes
     or marker mirrors.
   - If a real fixture shows Highlights splits an image's note into a distinct, mechanically identifiable adjacent
     shape, add a narrow merge rule with a regression test. Otherwise keep this out of scope.

5. Keep identity stable.
   - Image block IDs continue to use source PDF path plus image bytes only.
   - Changing the image note/comment updates the rendered block content in place without tombstoning the old block or
     changing the asset filename.

## Implementation Plan

1. Add a failing regression fixture/test for the missing shape.
   - Start with a parser unit test for a TextBundle image annotation whose note is represented in Markdown image
     metadata and/or the real adjacent Highlights shape.
   - Add a CLI integration assertion alongside `highlights_ref_sync_renders_textbundle_image_selections` proving the
     generated reference note contains the image embed and the image note.

2. Upgrade the image parser data model.
   - Extend `MarkdownImage` / `SidecarImage` to retain optional title text in addition to target and alt text.
   - Replace the current "alt or target" annotation text fallback with an explicit image-note source builder so paths
     are never rendered as notes.

3. Render image notes through the existing comment callout path.
   - Populate `SidecarAnnotation.comment` and `task_source` for image notes using the combined source.
   - Reuse `push_annotation_comment_callout()` so visual output stays consistent with text highlights.

4. Preserve task extraction from image notes.
   - Add or extend tests showing a `#task` line in an image note creates the same source-linked task currently created
     for explicit image comments.
   - Keep routed-task behavior unchanged.

5. Update documentation and fixtures.
   - Clarify in `docs/highlights-ref-sync.md` that image annotation notes render as nested comment callouts.
   - Document the supported TextBundle image-note shapes.
   - Update `tests/fixtures/highlights_ref/image_sidecar.md` if useful as a readable contract fixture.
   - Add a short README note only if the top-level highlights summary needs to mention image annotation notes
     explicitly.

6. Verify.
   - Run `cargo fmt --check`.
   - Run focused tests: `cargo test highlights_ref`.
   - Run the relevant CLI integration test: `cargo test highlights_ref_sync_renders_textbundle_image_selections`.
   - If focused tests pass, run the broader `cargo test` if time is reasonable.

## Risks

- Alt text can be a caption rather than a user note. The implementation should avoid rendering generic fallback data and
  should prefer explicit/title note sources before using alt text.
- Highlights note-format settings are customizable. The tests should document the exact supported shapes, and
  unsupported shapes should fail by omission rather than by corrupting marker/sticky-note semantics.
- A broad "merge next note into previous image" rule is tempting but risky; only add it if the real export shape is
  unambiguous.

## Out of Scope

- New CLI options or subcommands.
- Garbage-collecting orphaned copied image assets.
- Writing image annotation notes back into PDFs.
- Changing text highlight, standalone note, or marker synchronization semantics.

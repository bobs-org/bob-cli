---
create_time: 2026-06-03 07:29:36
status: wip
prompt: sdd/prompts/202606/highlights_sidecar_style_1.md
---
# Highlights Linked Sidecar Support Plan

## Context

`bob highlights-ref` currently parses a deliberately simple Highlights Markdown sidecar format:

- page headings like `## Page 12`
- annotation separators like `---`
- highlights as blockquote lines
- comments as non-heading text after a highlight
- standalone notes as non-blockquote text, with the first standalone note skipped as the PDF marker mirror

The example sidecar at `~/tmp/example_highlights_sidecar.md` uses a different Highlights export shape:

```md
# 「highlights-ref-sync」

#### [Page 1](highlights://highlights-ref-sync#page=1)

##### 2026-06-03:

> Highlights Reference Note Sync

- status: wip
- parent: obsidian

---
```

It also includes wrapped highlight text where only the first physical line starts with `>`, and `***` separators between
annotations.

## Goal

Add support for this linked-page Highlights sidecar style while preserving the existing fixture-backed sidecar contract,
generated note format, block-ID stability for already-supported sidecars, marker/frontmatter synchronization, and
tombstone behavior.

No new CLI subcommands or options are needed.

## Implementation Plan

1. Extend page heading recognition in `src/native/highlights_ref/mod.rs`.
   - Keep existing support for `## Page 12`, `## p. 12`, and related heading text.
   - Recognize Markdown-link page headings such as `#### [Page 1](highlights://...#page=1)`.
   - Use the link label, e.g. `Page 1`, as the rendered page label instead of the full Markdown link.
   - Leave non-page headings like the document title and dated `##### 2026-06-03:` metadata headings ignored by the
     annotation parser.

2. Teach blockquote parsing about Highlights hard-wrapped quote lines.
   - Continue treating one or more `>` lines as a highlight quote.
   - If a nonblank, non-heading line immediately follows quoted text without an intervening blank line, treat it as
     quote continuation text. This covers:
     ```md
     > It only writes the PDF marker when frontmatter is the selected source and --write-pdf is supplied.
     ```
   - Keep comment parsing as non-heading text after the quote/comment boundary, preserving the existing
     `Comment:`/`Note:` stripping behavior.
   - Be careful not to pull marker-list lines or explicit comment labels into quote continuation text.

3. Support the linked-style marker mirror.
   - In the existing simple format, the first standalone note is skipped as the marker mirror.
   - In the linked style, the marker mirror can appear as a blockquoted title followed by marker-list fields such as
     `- status:` and `- parent:`.
   - Add a narrow detector for this shape so that the first marker mirror annotation is excluded from generated
     highlights rather than rendered as a highlight with a comment.
   - Keep this logic local to sidecar parsing/rendering and do not change the authoritative PDF marker-note parser.

4. Preserve generated output semantics.
   - Keep rendering generated content under `<!-- highlights:begin -->` / `<!-- highlights:end -->`.
   - Keep block IDs based on source PDF, page label, annotation kind, ordinal-on-page, and normalized annotation text.
   - Do not include highlight comments in the block-ID identity.
   - Preserve removed-highlight tombstones for IDs present in the existing note but absent from the current sidecar.

5. Update documentation and fixtures.
   - Update `docs/highlights-ref-sync.md` to describe both supported sidecar shapes.
   - Add or update a fixture documenting the linked-page style, ideally based on `~/tmp/example_highlights_sidecar.md`.
   - Keep the current simple sidecar fixture documented because existing tests and users may rely on that format.

## Test Plan

1. Add focused parser/unit coverage where practical.
   - `sidecar_page_heading` extracts `Page 1` from `#### [Page 1](highlights://...#page=1)`.
   - Wrapped quote continuation lines are included in the highlight text.
   - Dated metadata headings are ignored.
   - A blockquoted marker mirror with `status` and `parent` fields is not emitted as generated highlight content.

2. Add an end-to-end `bob highlights-ref sync` integration test using the linked-page sidecar style.
   - Create a synthetic PDF marker note with `status` and `parent`.
   - Write a `.md` sidecar matching the example shape.
   - Assert the generated note includes `highlights_sidecar`, `highlights_count: 2`, and rendered sections for `Page 2`
     and `Page 6`.
   - Assert the Page 2 wrapped quote renders both lines as blockquote content.
   - Assert the Page 6 trailing text renders as a highlight comment.
   - Assert the Page 1 marker mirror is not rendered as a highlight or comment.

3. Run focused tests:

   ```bash
   cargo test highlights_ref
   ```

4. If focused tests pass quickly, run the full suite:
   ```bash
   cargo test
   ```

## Risks And Decisions

- The linked sidecar style does not require a new CLI option. Supporting it through format auto-detection keeps `sync`,
  `scan`, and `doctor` behavior unchanged.
- The marker-mirror detector should stay narrow to avoid misclassifying normal highlights with comments. It should
  require marker-list keys such as `status` and `parent`, and should only affect the marker-mirror skip path.
- Wrapped continuation parsing changes annotation text for newly supported linked-style sidecars. Existing simple-format
  sidecars should keep their parsed annotation text and block IDs unchanged.
- The sidecar parser currently returns annotations without surfacing parse errors. This plan keeps that behavior and
  avoids broad error-handling changes unless implementation reveals a real ambiguity.

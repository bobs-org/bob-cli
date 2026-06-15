---
create_time: 2026-06-15 10:53:03
status: done
prompt: sdd/prompts/202606/highlights_textbundle_creation_bug.md
---
# Plan: Document Highlights TextBundle Creation Bug

## Context

`bob highlights scan` recursively processes PDFs under the configured Bob library. The command currently discovers
sidecars by looking for `<pdf-basename>.md` first, then `<pdf-basename>.textbundle/text.md` or `text.markdown`. Image
annotations require a TextBundle sidecar because the Markdown image links need local `assets/...` files relative to the
sidecar text file.

The current code behavior is intentionally consumer-only:

- If `<pdf-basename>.textbundle` does not exist, `bob highlights` treats the PDF as having no sidecar and still performs
  marker/frontmatter planning.
- If the TextBundle directory exists but lacks `text.md` or `text.markdown`, the command reports an unsupported
  TextBundle sidecar.
- If the sidecar text references an image asset that is missing, the command reports an image-asset error.

The observed Highlights app issue is narrower than the Bob CLI contract: Highlights can update an existing TextBundle
correctly, but for PDFs with image annotations it may fail to create the TextBundle directory in the first place. The
practical workaround is to manually export/create the TextBundle once, in the same directory as the PDF and with the
exact PDF basename. After that, Highlights appears to keep the bundle updated.

The user referred to `bob highlight scan`, but the implemented and documented command is `bob highlights scan`;
documentation changes should use the canonical plural command name.

## Goal

Document the Highlights app TextBundle creation bug and the required workaround so users understand why image
annotations may be missing from `bob highlights scan` output even when Highlights autosave is enabled.

## Non-Goals

- Do not change `bob highlights` command behavior, flags, parser behavior, or output format in this task.
- Do not add a new CLI subcommand or option, so `memory/long/cli_rules.md` does not need to be reviewed for this
  documentation-only change.
- Do not make Bob create or mutate Highlights TextBundle sidecars; Bob should continue to treat PDF/sidecar content as
  user/app-owned input.
- Do not diagnose every possible Highlights export variant beyond the known TextBundle creation failure.

## Documentation Design

1. Update `docs/highlights-ref-sync.md` in the generated body and MacBook setup areas.
   - Keep the existing sidecar discovery contract intact.
   - Add a short "known Highlights app bug" note explaining that TextBundle export may need to be created manually once
     for PDFs with image or area annotations.
   - Spell out the exact expected path: `~/bob/lib/<ref_type>/<name>.textbundle/text.md`, with image files under
     `~/bob/lib/<ref_type>/<name>.textbundle/assets/`.
   - Make clear that the TextBundle basename must match the PDF basename and sit beside the PDF, for example
     `example.pdf` and `example.textbundle/`.
   - Explain that after the initial manual export, Highlights can update the existing TextBundle, so the workaround is a
     one-time creation step per PDF that needs image annotations synced.

2. Update the scan/setup guidance in `docs/highlights-ref-sync.md`.
   - Tell users to run `bob highlights scan --dry-run` after creating the TextBundle and confirm the output shows the
     expected `sidecar:` path and image counts.
   - Explain the user-visible symptoms: a missing TextBundle may appear as no sidecar and therefore no generated image
     annotations, while malformed or incomplete bundles can surface as `unsupported textbundle sidecar` or
     `image asset not found`.

3. Update the expected-failures table in `docs/highlights-ref-sync.md`.
   - Broaden the `image asset not found` fix from "re-export as TextBundle" to "manually create/export the TextBundle
     once, beside the PDF, then rerun."
   - Broaden the `unsupported textbundle sidecar` fix to mention verifying `text.md` or `text.markdown` inside the
     bundle directory.

4. Add a concise README pointer.
   - In the `bob highlights` overview, add one or two sentences after the sidecar paragraph that summarize the
     TextBundle requirement and link readers to the full docs.
   - Keep the README compact; the detailed bug/workaround belongs in `docs/highlights-ref-sync.md`.

## Verification

- Review the changed prose for command-name consistency: `bob highlights scan` everywhere, not `bob highlight scan`.
- Use `rg` to confirm the docs mention the exact TextBundle paths and the known creation workaround.
- No Rust tests are required for a documentation-only change. If any source, tests, CLI help, or behavior changes
  accidentally become necessary, stop and revise the plan first.

## Acceptance Criteria

- A user with `~/bob/lib/books/example.pdf` and image annotations can read the docs and know to create/export
  `~/bob/lib/books/example.textbundle/text.md` plus `~/bob/lib/books/example.textbundle/assets/...` before expecting
  images to appear in `bob highlights scan --dry-run`.
- The docs clearly attribute the first-create problem to the Highlights app, not to `bob highlights`, while still
  explaining how the Bob CLI behaves when the bundle is missing or incomplete.
- The docs preserve the existing Markdown sidecar/TextBundle discovery contract and do not imply Bob owns or creates
  sidecar files.

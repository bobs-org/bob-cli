---
create_time: 2026-06-03 05:17:57
status: wip
prompt: sdd/prompts/202606/highlights_ref_contract_1.md
---
# Plan: Tighten `bob highlights-ref` Reference Metadata Contract

## Context

`bob highlights-ref` currently parses the first standalone PDF note as a marker list, requires only `status`, and
injects a fallback `parent` value of `[[obsidian]]` through `--default-parent`, `BOB_HIGHLIGHTS_DEFAULT_PARENT`, or the
built-in default. Generated reference notes under `~/bob/ref/*.md` do not currently get a command-managed `type`
property.

The new desired contract is:

- Generated reference notes always contain `type: "[[ref]]"`.
- PDF marker notes must contain both `status` and `parent`.
- The marker `parent` value is the source for note frontmatter `parent`.
- `[[obsidian]]` should no longer be injected as a fallback parent.
- Examples and fixtures should use `status: wip` for PDFs that are started but not finished, instead of
  `status: reading`.

## Design

Treat `status` and `parent` as required synced marker/frontmatter fields, and treat `type` as command-managed note
frontmatter rather than a marker-synced field.

`type` should be rendered into every reference note as `[[ref]]`, should replace any existing `type` frontmatter during
sync, and should be excluded from marker round-trip behavior. If a PDF marker attempts to set `type`, reject it as a
command-managed field instead of letting it override the generated note type.

Remove the default-parent configuration path from the active highlights-ref surface because the marker is now
authoritative for `parent`. That means removing the config field, CLI option, environment fallback, printed config
report line, doctor check, README entry, and highlights-ref docs sections that describe fallback parent behavior. Keep
`parent` path handling simple: whatever valid marker/frontmatter value is supplied is rendered through the existing
frontmatter value renderer.

Do not add enum validation for `status`. The command should preserve user status strings. The `reading` to `wip` change
is a usage/docs/fixture update, not a migration that rewrites existing marker values.

## Implementation Steps

1. Update the marker/frontmatter projection contract in `src/native/highlights_ref/mod.rs`.
   - Replace the single required marker key with required keys for `status` and `parent`.
   - Validate missing or empty values for both marker-sourced and frontmatter-sourced sync.
   - Remove `projection_with_default_parent` and stop inserting fallback parent values in marker and frontmatter
     projections.
   - Remove `default_parent` from `Config`, config reporting, doctor checks, and CLI config args.

2. Add generated reference-note `type`.
   - Introduce constants for `type` and the fixed value `[[ref]]`.
   - Render `type: "[[ref]]"` into generated note frontmatter on every sync.
   - Treat existing `type` frontmatter as managed so it is replaced, not duplicated or preserved with a stale value.
   - Exclude `type` from marker/frontmatter projections and reject it in marker input as command-managed.

3. Update user-facing docs and fixtures.
   - Change marker examples to include both `status: wip` and `parent`.
   - Remove `BOB_HIGHLIGHTS_DEFAULT_PARENT` and `--default-parent` guidance.
   - Update doctor/help descriptions that mention default parent checks.
   - Update troubleshooting for missing required marker keys to mention both `status` and `parent`.
   - Update `tests/fixtures/highlights_ref/marker_note.txt`.

4. Update tests.
   - Adjust existing highlights-ref integration tests so all valid marker notes include `parent`.
   - Change active-reading sample statuses from `reading` to `wip`.
   - Assert generated notes include `type: "[[ref]]"`.
   - Add or revise failure coverage for missing `parent` and, if practical, marker `type` rejection.
   - Update unit tests for projection behavior, marker rendering order, and frontmatter rendering to match the new
     no-fallback contract.

5. Verify.
   - Run `cargo fmt --check` after formatting.
   - Run targeted tests for highlights-ref behavior if useful while iterating.
   - Run `cargo test`.
   - Run `cargo clippy --all-targets --all-features` if time permits, or the repository `just all` target if the test
     cycle remains practical.

## Compatibility Notes

Existing PDFs whose marker note lacks `parent` will fail with a clear missing required marker key error until the marker
is updated. Existing generated notes without `type` will be updated on the next successful sync. Existing marker and
frontmatter hashes should remain stable when an old generated note already has the same effective `parent` value and the
PDF marker is updated to include that value explicitly, because `type` is not part of the synced marker projection.

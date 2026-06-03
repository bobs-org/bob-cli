---
create_time: 2026-06-03 07:12:14
status: done
prompt: sdd/prompts/202606/highlights_ref_nested_lib_1.md
---
# Plan: Support Nested `bob highlights-ref` Library Reference Types

## Current Findings

`bob highlights-ref scan` already walks the configured library directory recursively. The current gap is not PDF
discovery itself; it is that each discovered PDF is mapped to a flat target note path by basename only:

- `~/bob/lib/books/systems-performance.pdf` currently writes `~/bob/ref/systems-performance.md`
- `~/bob/lib/papers/systems-performance.pdf` would collide with the same target note
- generated frontmatter has no path-derived `ref_type`

Sidecar discovery already looks next to the PDF with `pdf.with_extension("md")`, so a sidecar like
`~/bob/lib/books/systems-performance.md` should be found once the PDF is discovered. The needed behavior is to preserve
the nested library category in the generated reference note contract.

## Intended Contract

Treat the first path component under the configured library directory as the reference type:

```text
~/bob/lib/<ref_type>/<pdf_basename>.pdf
~/bob/lib/<ref_type>/<pdf_basename>.md
```

For PDFs under that shape, generate or update:

```text
~/bob/ref/<ref_type>/<pdf_basename>.md
```

Add command-managed frontmatter to generated reference notes:

```yaml
ref_type: <ref_type>
```

Top-level library PDFs remain supported for backward compatibility:

```text
~/bob/lib/<pdf_basename>.pdf -> ~/bob/ref/<pdf_basename>.md
```

For those legacy top-level PDFs, omit `ref_type` because there is no `<ref_type>` path component to derive. Explicit
`sync <pdf>` calls outside the configured library directory should also keep the existing flat fallback and omit
`ref_type`.

## Implementation Approach

1. Add a small path helper in `src/native/highlights_ref/mod.rs` that computes library-relative PDF metadata:
   - the relative PDF path under `config.lib_dir`, when available
   - the target note relative path with the extension changed to `.md`
   - the optional `ref_type`, taken from the first parent path component below `lib_dir`

2. Change `ref_note_path()` to use that helper:
   - nested library PDFs map to `config.ref_dir/<relative_pdf_path_with_md_extension>`
   - legacy top-level and out-of-library PDFs keep the existing `config.ref_dir/<stem>.md` fallback
   - keep existing validation for invalid/non-UTF-8 basenames, and add a clear error if a derived `ref_type` is not
     UTF-8

3. Add `ref_type` as a command-managed frontmatter field:
   - render it from the path-derived metadata, not from marker/frontmatter projection
   - exclude it from marker/frontmatter sync and marker hashes
   - reject `ref_type` in the PDF marker note with the existing command-managed field error style
   - replace stale existing `ref_type` values on sync when the PDF path has a derived type

4. Preserve existing safety behavior:
   - collision detection still runs before writes, but now detects collisions by the new nested target note paths
   - atomic writes already create parent directories, so nested `ref/<ref_type>/` note directories should work without
     new write plumbing
   - dirty-target checks should continue to inspect the actual note path selected by the plan

5. Update docs:
   - `docs/highlights-ref-sync.md`
   - README highlights-ref sections
   - examples should show `~/bob/lib/<ref_type>/<pdf_basename>.pdf` and sidecar `.md` mapping to
     `~/bob/ref/<ref_type>/<pdf_basename>.md`
   - document that `ref_type` is path-derived and command-managed

## Test Plan

Update and add focused tests in `tests/cli.rs`:

- add or update a `sync` test with `lib/books/systems-performance.pdf` and `lib/books/systems-performance.md`
  - assert the note is written to `ref/books/systems-performance.md`
  - assert `ref_type: books`
  - assert `source_pdf` and `highlights_sidecar` keep the nested `lib/books/...` paths
  - assert the old flat `ref/systems-performance.md` target is not created

- update the recursive `scan` integration test to expect nested notes under `ref/books/` and `ref/papers/`, including
  `ref_type` frontmatter

- replace the existing duplicate-basename collision expectation with two cases:
  - same basename in different `ref_type` directories succeeds and writes distinct notes
  - a real same-target collision still fails before writes, for example two PDFs in the same library-relative directory
    that map to the same `.md` target

- add unit coverage for the path helper:
  - top-level `lib/example.pdf`
  - nested `lib/books/example.pdf`
  - deeper nested path if supported by the helper
  - out-of-library explicit PDF fallback

Run targeted verification first:

```bash
cargo test highlights_ref
```

Then run the broader suite if the focused tests pass:

```bash
cargo test
```

## Open Decisions

I will implement `ref_type` as the first directory component under `lib_dir`, while preserving the full relative library
path in the output note path. That supports the stated one-level shape and does not create avoidable collisions if
deeper subdirectories appear later.

No CLI subcommands or options are needed, so help ordering should not change.

# Task-drawer vectors

`vectors.json` is the Rust-owned conformance source for Bob's task drawer
reader and the pure drawer-to-H2 title mapping in `src/native/capture_drawers.rs`.
Plugin twins update bob-cli first, then copy this file byte for byte.

## Shape

- `schema_version`: `1`.
- `cases[]`: `DV` recognition vectors. `source_lines` plus optional `eol`
  (`crlf`) form the complete note. `owner` is a task block ID. `expect[]`
  lists each recognized drawer's authored `title`, Unicode uppercase `key`,
  source `marker`, one-based `line`, and optional half-open UTF-8 byte
  `header_range` and `title_range` in the complete note. Empty expectations
  prove the listed lookalikes stay notes.
- `body_cases[]`: pure body-shape vectors. `marker` is `bold` or
  `legacy_caps`; `excluded: true` means the body is not drawer-shaped.
- `heading_mappings[]`: `DH` round-trip vectors for verbatim titles and the
  empty H2 section.

The header range excludes LF or CRLF. The title range excludes the list marker,
bold delimiters, and heading prefix. Ranges are byte offsets, not character
indexes.

# bob-obsidian-assets

Embedded font and vendor payloads for Bob (`bob-obsidian`).

This crate owns the byte-identical payloads Bob embeds:

- `assets/print_fonts/` — pinned static print faces, OFL notice, and provenance README.
- `scripts/web_clip/fonts/` — web-clip reader font notices, DejaVu Sans, and README.
- `scripts/web_clip/vendor/` — vendored Defuddle and Mermaid bundles, licenses, and README.

The library exposes each payload as a `&'static [u8]` constant backed by
`include_bytes!`. The root package re-exports the print faces through
`src/print_fonts.rs` and references the remaining payloads from
`src/scripts.rs`; installed paths and bytes are unchanged.

Versioned independently from the root package. Future payload or dependency
bumps belong to release-plz; do not edit the version by hand.

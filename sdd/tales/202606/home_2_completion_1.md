---
create_time: 2026-06-03 04:15:54
status: done
prompt: sdd/prompts/202606/home_2_completion_1.md
---

# Complete home-2 Highlights Reference Sync Verification

## Context

The `home-2` epic is open while all child phase beads are closed. Source review of the linked bob-cli implementation
found one remaining correctness gap: a frontmatter-to-marker `sync --write-pdf` run without a sidecar can write the
reference note with the PDF hash captured before the marker write. That leaves `source_pdf_sha256` stale until a second
run.

## Plan

1. Add a regression test in bob-cli for frontmatter-to-marker sync that verifies the generated note records the
   post-write PDF SHA-256 after `--write-pdf`.
2. Update the highlights-ref execution path so note rendering recomputes pipeline metadata after any PDF marker write,
   even when there is no Highlights sidecar.
3. Run targeted and full bob-cli release checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features`,
   `cargo test`, and `just check-scripts`.
4. Update the epic plan frontmatter status to `done` after verification.
5. Close the `home-2` epic bead with `sase bead close home-2`.

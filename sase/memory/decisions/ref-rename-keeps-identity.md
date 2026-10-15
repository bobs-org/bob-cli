---
summary: Ref renames change bundle paths while preserving reference and task identity.
---

# Ref Rename Keeps Reference Identity

*Accepted · project*

**Applies to.** bob-cli, bob-mac-capture, bob-plugins.

**Claim.** `bob ref rename` changes the file stem for a modern reference bundle:
the note, PDF, sidecar, audio, and vault links to those files. It keeps the
frontmatter `id`, the reading task's `^ref` block ID, `title`, `source_url`, the
PDF marker, and the PDF bytes. V1 refuses annotated references. If annotated
renames are needed later, the pipeline will own an identity-origin frontmatter
field so it can preserve highlight IDs instead of re-keying `^h-` anchors.

**Why.** Changing `id` or `^ref` requires writing the PDF marker and churns the
Today, Task Links, and Depends-On joins. Re-keying highlights would require
rewriting anchors, assets, inbound fragments, and archived `[h::]` digests.
Matching by content would add fuzzy matching to every scan. The file stem is
therefore a locator; it is not the reference's identity.

**Cost.** The stable `id` may differ from the filename stem; six existing
references already have that shape.

**Reopens when.** An annotated reference rename is needed.

Evidence: research report
`research:202610/bob_ref_rename_command/bob_ref_rename_command.md`; epic
`bob-cli-7x` (`plan:202610/bob_ref_rename.md`); live writer and verification
phase `bob-cli-7x.4` (`docs/ref.md`, `tests/cli/ref_library/rename.rs`).

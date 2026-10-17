---
metadata:
  status: superseded-in-part
  superseded_by: decisions:reference-ids-follow-file-stems
summary: Renaming eligible references moves their bundle paths and inbound links while preserving reference and reading-task identity.
---

aka bob ref rename, reference rename, preserve ref identity

**Applies to.** bob-cli, Bob Mac Capture's Bob Refs rename flow, bob-plugins, and
the Bob vault's reference links.

**Claim.** Renaming an eligible modern, PDF-backed, unannotated reference changes
its file stem and the corresponding bundle paths in the same reference-type
directory. The bundle includes the note, PDF, and any same-stem sidecar or audio
companions; the rename also rewrites inbound vault links and the note's companion
paths. It keeps the reference's `id`, `^ref-…` block ID, title, `source_url`,
dates, PDF marker, and PDF bytes. The reading task therefore retains its identity
and links. V1 refuses legacy and intake references, annotated references, and
notes whose PDF sync plan is not clean. Refiling, retitling, bulk renames, and
intake renames are outside this contract.

**Why.** The `id` is the URL deduplication key and the `^ref-…` block ID joins the
reading task to the reference. Changing either would break identity joins and
churn Today, Task Links, and Depends-On links. The file stem is a locator, not the
reference's identity; six existing references already have an ID that differs
from the filename stem. A scratch-vault experiment showed that moving the PDF
and note, rewriting inbound links, and updating only `source_pdf` lets
`bob ref sync -d <new pdf>` plan no changes while `bob ref find <old id>` still
resolves. Note-only renames can make the next scan recreate the old note and
duplicate the reading task. Re-keying highlights would require rewriting
anchors, assets, inbound fragments, and archived `[h::]` digests.

Rejected alternatives:

- **Mint a new reference ID.** That breaks URL deduplication and turns the same source
  into a second reference.
- **Change the `^ref` block ID.** That disconnects existing task links from the reading
  task.
- **Rename annotated references in this version.** Highlight IDs and follow-up digests
  depend on paths, so changing those paths would require re-keying annotations.
- **Retitle or refile as part of rename.** A rename changes the stem only; metadata and
  the reference-type directory remain stable.

Evidence: research report
`research:202610/bob_ref_rename_command/bob_ref_rename_command.md`; plan
`plan:202610/bob_ref_rename.md` (identity invariants and scratch-vault experiment A);
epic `bob-cli-7x`, phases `bob-cli-7x.3`, `bob-cli-7x.4`, and `bob-cli-7x.7`
(`docs/ref.md` and `tests/cli/ref_library/rename.rs`).

**Cost.** File paths and inbound links change while identity metadata remains
stable, so clients must follow the report's new path without treating the new
filename as a new reference. Annotated references and directory moves remain
unsupported. Browser bookmarks and copied `obsidian://` or `highlights://` URIs
cannot be updated by the vault rename.

**Reopens when.** The reference or reading-task identity contract changes, or
annotated renames gain a safe method that preserves highlight IDs and follow-up
links.

Superseded in part: the claim that a rename keeps the reference `id`, PDF marker, and PDF bytes is replaced by [[decisions:reference-ids-follow-file-stems]]; the `^ref-…` block ID, title, `source_url`, and dates still stay.

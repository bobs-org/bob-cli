---
keyword: Reference Ids Follow File Stems
aliases:
  - reference id follows stem
  - ref id rename
  - id follows filename
summary: A reference id follows its note stem, with a numbered suffix only to avoid duplicate ids across ref/.
origin: requested
scope: project
metadata:
  status: accepted
---

**Applies to.** bob-cli, Bob Mac Capture's rename card, and the Bob vault.

**Claim.** A reference's recorded `id` equals its note stem. The only exception is a
`_N` suffix that avoids a duplicate across `ref/`. `bob ref rename` re-keys the id in
frontmatter and the PDF marker with the stem; a same-stem rename repairs a mismatch.
The reading task's `^ref-…` block ID, title, `source_url`, and dates stay unchanged.
References without an id stay id-less.

**Why.** Bryan's rule is that “a reference's id, which is recorded in the PDF's
page-1 marker and in the note's frontmatter `id`, must equal the reference note's
basename. The only allowed exception is avoiding a duplicate id. Ids must be distinct
across the whole `ref/` directory.” The id is a lookup and display key; URL deduplication
uses `source_url`, so re-keying it breaks no identity join. The sync engine already
propagates a note-side id edit to the PDF marker and refreshes sync bookkeeping.

Rejected alternatives:

- **Keep the stale id.** This preserves the previous claim that rename identity lives in
  the id even though the file stem is the required id.
- **Re-key the `^ref` block ID too.** That would churn Task Links, `[id::]`, and
  Depends-On links that join on the block ID.
- **Refuse when another reference uses the stem id.** A numbered suffix preserves the
  rule and leaves every id unambiguous.

**Cost.** An id change rewrites the PDF bytes and makes `bob ref find <old id>` stop
resolving. A PDF left open in Highlights could later write back its stale marker.

**Reopens when.** Reference ids gain another consumer, or the reading-task block ID
must follow the file stem.

Evidence: `plan:202610/ref_rename_id_follows_stem.md`, `docs/ref.md`, and
`tests/cli/ref_library/rename.rs`.

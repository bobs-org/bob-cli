---
create_time: 2026-06-07 11:13:29
status: done
prompt: sdd/prompts/202606/highlight_task_source_links.md
---
# Plan: Link Annotation-Created Tasks Back to Their Source Highlight Block

## Context

`bob highlights` (in `src/native/highlights_ref/mod.rs`) already creates top-level Obsidian tasks from `#task` bullets
found in PDF highlight comments and standalone sticky notes. As of commit `848318a`, a source bullet like:

```md
- #task Compare this claim with the appendix.
```

becomes a sibling task immediately under the generated PDF reading-status `^task` line:

```md
- [ ] #task Compare this claim with the appendix. [created::2026-06-07]
```

Separately, every rendered highlight/note in the managed `<!-- highlights:begin -->` … `<!-- highlights:end -->` region
already carries a **stable Obsidian block ID** of the form `^h-xxxxxxxxxxxx`, produced by `annotation_block_id()` (a
deterministic SHA-256 over the source PDF path, annotation kind, page label, per-page ordinal, and normalized text). For
example:

```md
> Highlighted claim.
>
> [comment] #task Compare this claim with the appendix. ^h-2b91f0a4c7de
```

**The gap:** the created task has no connection to the highlight/comment that spawned it. The user wants each created
task to link back to that source block — beautifully.

This is behavior inside the existing `bob highlights sync` / `scan` flow. No new CLI surface, so
`memory/long/cli_rules.md` is not required.

## Goal

When an annotation `#task` bullet creates a task, embed an Obsidian **block backlink** in the created task line pointing
at the `^h-…` block of the highlight comment or sticky note that triggered it. Clicking the link in Obsidian jumps
straight to the highlighted passage / note in the same reference note.

The link must be:

- **Beautiful** — compact and unobtrusive, not a raw `#^h-…` id smeared across the line.
- **Resolvable** — the `^h-…` target always exists in the same note's managed Highlights region.
- **Idempotent** — adding the link must not change duplicate detection; re-syncing must never recreate or mutate an
  already-created task.

## Design Decision: How the Link Renders (leading the design)

The block lives in the _same_ note as the task, so a same-file block reference `[[#^h-…]]` is the correct,
self-contained target (no note basename to drift if the note is renamed). A **bare** `[[#^h-2b91f0a4c7de]]` renders its
raw id as link text in Obsidian — ugly. The fix is an **alias**.

**Recommended (Option A — bookmark glyph):** a trailing aliased link using a 🔖 glyph, placed after the task prose and
before the `[created::]` field so provenance sits with the statement and the date stays last:

```md
- [ ] #task Compare this claim with the appendix. [[#^h-2b91f0a4c7de|🔖]] [created::2026-06-07]
```

Obsidian renders this as: `☐ #task Compare this claim with the appendix. 🔖 created∷ 2026-06-07`, where 🔖 is a
one-glyph clickable jump to the source highlight. Minimal visual noise, unmistakable affordance.

**Alternative (Option B — text alias):** identical mechanics, a word instead of a glyph for users who prefer no emoji:

```md
- [ ] #task Compare this claim with the appendix. [[#^h-2b91f0a4c7de|source]] [created::2026-06-07]
```

**Rejected (Option C — Dataview `[source:: …]` inline field):** `… [created::2026-06-07] [source:: [[#^h-…]]]`. Reads
fine but the existing task-property stripper (`strip_matching_obsidian_task_properties`) does not handle the nested `]]`
inside a property value and would leave dangling brackets in the identity key. Avoiding nested brackets keeps
idempotency robust.

I am proposing **Option A** as the default. The glyph vs. word choice is the one purely-aesthetic knob; it is a single
constant (`SOURCE_LINK_ALIAS`) and trivial to flip at review time.

## Non-Goals

- No new subcommand, option, environment variable, or interactive prompt.
- Do not change the generated PDF `^task` reading-status line, its semantics, or its block ID.
- Do not back-fill links onto tasks created _before_ this change (link-less existing tasks are preserved as-is; see
  "Backward compatibility"). A future enhancement could enrich them, but rewriting existing user task lines is out of
  scope here.
- Do not change how highlights/comments/notes themselves render in the managed region.
- Do not change duplicate-detection _semantics_: two byte-identical `#task` bullets across different highlights still
  collapse to one task (today's behavior). This feature adds provenance, not disambiguation. (See "Considered &
  deferred".)

## How It Works

### 1. Carry the source block ID on each candidate

`AnnotationTaskCandidate` currently holds `{ identity, task_text }`. Add `source_block_id: String` (the bare `h-…` id,
no leading `^`).

The block ID requires `config` + `pdf` (via `source_pdf_value`), which `annotation_task_candidates()` does not currently
receive. Change its signature to `annotation_task_candidates(config, pdf, sidecar)`.

Crucially, `annotation_task_candidates()` and `render_sidecar_highlights()` already iterate `sidecar.annotations` in the
**same order** and **skip the same first marker mirror** (`is_sidecar_marker_mirror`). So computing
`annotation_block_id(config, pdf, annotation)` inside the candidate loop yields exactly the id that the managed region
renders for that annotation. Attach that id to every candidate derived from the annotation (multiple `#task` bullets in
one comment → multiple tasks, all pointing at the same source block — the correct outcome).

`annotation_task_candidate_from_source_line` / `annotation_task_candidates_from_text` will take the block id as a
parameter (or have it attached by the caller) so the bare candidate stays decoupled from hashing.

### 2. Render the backlink in the created line

In `insert_missing_annotation_tasks`, build the missing line as:

```
- [ ] {task_text} [[#^{source_block_id}|{SOURCE_LINK_ALIAS}]] [created::{date}]
```

A guard: only emit the link when `source_block_id` is non-empty (defensive; in practice every non-marker annotation has
one).

### 3. Keep the link out of identity (idempotency)

Duplicate detection compares **normalized identity**, not raw lines. The candidate's identity is derived from the source
bullet, which has no link. The _existing_ created line now does have a link, so identity computation on existing lines
must strip it, or re-syncs would see a mismatch and duplicate the task.

Add a small `strip_source_block_link(text)` helper that removes any `[[ … ]]` wikilink whose target contains a block
reference (`#^`) — covering `[[#^h-…]]`, `[[#^h-…|🔖]]`, and the full `[[note#^h-…|…]]` form, while never touching a PDF
wikilink (which has no `#^`). Call it inside `annotation_task_identity()` alongside the existing
`strip_obsidian_task_properties()` step, so **both** the candidate side (no-op) and the existing-line side (strips the
injected link) compute the same identity. Net identity for the example stays
`#task Compare this claim with the appendix.` exactly as today.

This also means the new line is never mistaken for the generated PDF `^task` line: it contains no `^task` whitespace
token, so `parse_pdf_task_line` and `existing_annotation_task_identity`'s `^task` guard both behave unchanged.

### 4. Wire into `plan_pdf_sync`

Only the call site changes: `annotation_task_candidates(sidecar.as_ref())` →
`annotation_task_candidates(config, pdf, sidecar.as_ref())`. `render_body` runs first (so the `^h-…` blocks are already
in the managed region of the body), then `insert_missing_annotation_tasks` adds the linked tasks after `^task`. A
same-file forward link resolves regardless of ordering. Dry-run / scan continue to report `note_action: update`
naturally when new linked tasks would be created.

## Backward Compatibility

Tasks created by the prior commit have no link. On the next sync:

- existing-line identity = (strip checkbox, strip properties, strip-link no-op, normalize) = `#task …`
- candidate identity = `#task …`

→ they match → the old task is **not** recreated and is left exactly as-is (we never rewrite existing task lines). New
bullets get linked tasks; pre-existing tasks stay link-less. No duplication, no churn.

## Considered & Deferred: identity disambiguation by block ID

Including the source block id _in_ the identity would let two byte-identical `#task` bullets on different highlights
produce two distinct linked tasks (arguably more correct). It is deferred because it reintroduces a migration edge
(link-less pre-existing tasks would no longer match and could duplicate) and multiplicity bookkeeping that outweighs the
benefit for the common case (distinct task text). Today's collapse-identical behavior is preserved; this can be
revisited later without breaking the link format.

## Implementation Steps

1. Add `source_block_id: String` to `AnnotationTaskCandidate`; add a `SOURCE_LINK_ALIAS` constant.
2. Thread `config` + `pdf` into `annotation_task_candidates`; compute `annotation_block_id` per non-marker annotation
   and attach to its candidates.
3. Update `insert_missing_annotation_tasks` to render `[[#^{id}|{alias}]]` between the task prose and `[created::]`.
4. Add `strip_source_block_link` and call it in `annotation_task_identity` so the link is excluded from identity on both
   sides.
5. Update the `plan_pdf_sync` call site to pass `config, pdf`.
6. Update docs (`README.md` §highlights, `docs/highlights-ref-sync.md` §annotation tasks) to show the linked format and
   explain that the link is a same-file block backlink to the source highlight/note and is ignored for duplicate
   detection.

## Tests

- **Unit — candidate carries the right id:** extend `annotation_task_candidates_extract_from_comments_and_notes` to call
  the new signature with a `Config` + fake pdf (pattern from `pdf_path_metadata_derives_nested_reference_paths` at
  mod.rs:5416) and assert each candidate's `source_block_id` equals `annotation_block_id` for its annotation — same id
  for two bullets in one comment, distinct ids across different annotations.
- **Unit — link rendered + idempotent + identity ignores link:** extend
  `annotation_task_insertion_is_idempotent_and_preserves_existing_states` so candidates carry block ids; assert the new
  line contains `[[#^h-…|<alias>]] [created::…]`, that a re-run is byte-stable, that a pre-existing _link-less_ `#task`
  line with the same text is not recreated (backward compat), and that completed/cancelled linked tasks are preserved.
- **CLI — end-to-end resolvability:** update `highlights_ref_sync_creates_tasks_from_pdf_note_task_bullets`
  (tests/cli.rs:4114). The created lines now carry links, so assert each created task contains its prose plus `[[#^h-`
  and `[created::]`, extract the `^h-…` id from the task line, and assert that exact `^h-…` block exists in the managed
  Highlights region (the link resolves). Keep the existing idempotency and completed/cancelled re-sync assertions.

## Verification

```bash
cargo fmt --check
cargo test annotation_task
cargo test highlights_ref_sync_creates_tasks_from_pdf_note_task_bullets
cargo test highlights_ref
```

Tests must compute the expected local `[created::]` date and the expected `^h-…` ids rather than hard-coding, while docs
keep concrete example ids/dates.

## Risks

- **Alias taste.** The 🔖 glyph is a one-constant decision; swap to `source` text if preferred.
- **Block-id drift on edits.** Editing the highlight text changes its `^h-…` id (old block becomes a tombstone, new
  block gets a new id). A previously created task keeps its original link, which now targets the tombstoned block —
  still a valid, resolvable jump (rendered under `### Removed highlights`). No new task is created unless the _task_
  text changes. This matches the existing edit-creates-new-task philosophy and is worth a one-line doc note.
- **Same-file link form.** `[[#^h-…]]` is the standard Obsidian same-file block ref and is rename-safe; using the
  bare-id form (no note basename) avoids basename drift entirely.

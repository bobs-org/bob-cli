---
keyword: Relation Label Rename Rewrites History
aliases:
  - relations rename
  - label rename transaction
summary:
  "bob relations rename rewrites matching label spans everywhere, including
  done/ and the previous daily note, in one previewed transaction and one
  revertable commit."
metadata:
  status: accepted
  decided: 2026-10-24
---

**Applies to.** bob-cli `bob relations rename`, the Mac card Ctrl+R rename
mode, and the Obsidian relation-label stage rename mode.

**Claim.** A rename runs one previewed transaction and makes one revertable
commit (`bob relations rename: OLD → NEW`). It rewrites only matching label
spans on accepted LINKS and LINKED FROM lines across the whole catalog scope,
including `done/` and the previous daily note — the recorded LR7 exception:
label spans only, never reopening tasks, minting IDs, pruning edges, or
reordering entries. Planning refuses (`malformed_links_line`, `stale_note`)
rather than under-covering. Preview lists every file and changed line, link
and mirror counts including `done/`, hidden-state and inverse changes, and
NEW's `replacement` forms. Undo is `git revert` of the one commit.

**Why.** Renaming without touching `done/` would leave history disagreeing
with the catalog and resurface OLD in suggestions via `renamed_reappeared`
forever. Rewriting without a single commit would make the rename
un-revertable. Covering malformed lines silently would corrupt notes the
rescan cannot verify. The shared `vault_rewrite` transaction (extracted from
`bob themes rename`) gives lock, pre-sync, snapshot, stale check, atomic
write with rollback, verify, one commit, and post-sync in one place. Rejected
alternatives:

- **Exclude done/ and old dailies** — history keeps the old spelling and the
  catalog never converges.
- **Rewrite without a commit** — no atomic undo; `git revert` stops working.
- **Skip malformed lines silently** — unverifiable partial coverage; refuse
  instead.
- **Journal-only alias** — OLD text stays in notes, so every scan keeps
  counting it.

Evidence: `plan:202610/relation_label_vocabulary.md` RV8;
`research:202610/relation_card_label_vocabulary`;
`docs/task-relations.md` §16; commits `fd9380cb`, `4c46dadd`.

**Cost.** Malformed or concurrently changed targets refuse instead of
renaming. Offline runs refuse dirty paths. Clients must render the preview
and merge arming from bob's JSON, never their own grammar.

**Reopens when.** A rename must touch more than label spans, must cover
unreadable notes, or `done/` must become fully read-only again.

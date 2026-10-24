---
keyword: Relation Label Prefs Live In A Per-Host Journal
aliases:
  - relation journal
  - relation hide journal
summary:
  "Relation label hide, unhide, and rename events live in vault-synced
  .bob/relations/<host>.jsonl; fold is last-writer-wins and bob is the only
  writer."
metadata:
  status: accepted
  decided: 2026-10-24
---

**Applies to.** bob-cli, Bob Mac Capture, and Obsidian plugins that hide,
unhide, or rename relation labels.

**Claim.** Preferences live in `.bob/relations/<host>.jsonl`, one
append-only file per host inside the vault so Git syncs them. `host` is the
same label vault-sync uses in commit subjects. Events are `v: 1`
`relation_label.hide`, `relation_label.unhide`, and `relation_label.rename`
(`from`, `to`, plus display names), each with `v`, `ts`, `host`, and `via`.
Fold is exactly the theme rule, implemented once in the shared `pref_journal`
module: read every `*.jsonl` file, skip malformed lines with a bounded
warning, sort by `(ts instant, host, path, line)`, and let the last writer win
per key. A rename moves the source key's explicit state unless the target
already has state; events after a rename on the old key apply to the old key.
bob is the only writer; other clients call bob. This is the twin of
[[decisions:theme-prefs-live-in-per-host-journal]].

**Why.** Frontmatter on daily notes would mix derived history with
preferences. A machine-local file would not sync hides across apollo and the
Mac. A single shared JSON file would lose concurrent host appends; the
append-only per-host journal plus union merge keeps both hosts' writes.
Rejected alternatives:

- **Daily-note frontmatter** — every hide would dirty today's ledger.
- **Machine-local store** — a hide on apollo would not reach the Mac.
- **One shared JSON object** — two hosts writing at once would clobber.

Evidence: `plan:202610/relation_label_vocabulary.md` RV3;
`research:202610/relation_card_label_vocabulary`;
`docs/task-relations.md` §16; commit `57d77612`.

**Cost.** Fold must skip malformed lines. Rename is a logged event plus a
note rewrite, not a journal-only alias. Writers serialize through bob under
`bob_sync.lock`.

**Reopens when.** Concurrent host folds mis-order clocks often enough to need
a vector clock, or preferences must live outside the vault.

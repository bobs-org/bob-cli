# Task-cancel vectors

`vectors.json` is the single source of truth for cancel-label engines in
bob-cli and bob-plugins (`docs/task-relations.md` §17). Its sections are
`cancel`, `prune`, `rule`, `key`, and `config`. Rust-only cases, when
any exist, are marked `rust_only`.

bob-plugins copies this file byte-for-byte. A behavior change edits
bob-cli first; never hand-edit the copy.

## Provenance

The `cancel` and `prune` bytes were generated from bob-plugins
`12d4c0e2a9de126ca2ba036bf459e0640f7d1657` (`12d4c0e`), by loading
`plugins/bob-navigation-hotkeys/main.js` the way
`scripts/navigation-hotkeys-harness.cjs` does and calling:

- `helpers.planTaskCancelBatch(content, session, { date, reason: "", fallbackReason })`
  after `helpers.discoverCountedObsidianTaskTargets(content, line, 0)`
- `helpers.planDeferredPomodoroLinkCleanup(dailyContent, targets, { dailyPath, noteIndex })`
  with `helpers.createScheduledRecoveryNoteIndex`

`rule`, `key`, and `config` are hand-authored from §17. Change them only
when §17 changes.

## Regenerating `cancel` and `prune`

Do not hand-edit those two sections. In a bob-plugins checkout at the
commit above (or a later commit whose Task Card Cancel planners are
still the authority):

1. Require `scripts/navigation-hotkeys-harness.cjs`.
2. For each `cancel` case, run `planTaskCancelBatch` with `reason: ""`
   and the case's `fallbackReason`.
3. For each `prune` case, build a note index from `notePaths` and run
   `planDeferredPomodoroLinkCleanup`.
4. Replace the `expected` objects with the planner output.
5. Review every generated postimage against §17 CL4. If nav's bytes
   disagree with CL4, stop and record it on the bead rather than
   encoding a surprise.
6. Update this README's bob-plugins commit.

## Section contracts

| Section | Pins |
| --- | --- |
| `cancel` | source-note preimage, line, date, `fallbackReason` → postimage, counts, recurring refusal |
| `prune` | daily preimage, block ids, note index → postimage and removed counts |
| `rule` | configured labels, label, previous label, action, direction, source state, new-task, modifiers, trigger → `verdict` and `cancels` |
| `key` | `relation_label_key` matching against configured cancel labels |
| `config` | YAML text → effective keys, warning count, and `linked_from_lines` when relevant |

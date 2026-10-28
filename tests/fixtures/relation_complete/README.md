# Relation-complete vectors

`vectors.json` is the single source of truth for complete-label engines in
bob-cli and bob-plugins (`docs/task-relations.md` §18). Its sections are
`done`, `rule`, `key`, and `config`. Rust-only cases, when any exist, are
marked `rust_only`.

bob-plugins copies this file byte-for-byte. A behavior change edits
bob-cli first; never hand-edit the copy.

## Provenance

The `done` bytes were generated from bob-plugins
`e39572101761a1dbb85ca023c65bdc61fa58b3b6` (`e395721`), by loading
`plugins/task-status-cycler/main.js` the way
`scripts/task-status-cycler-harness.cjs` does and calling:

- `helpers.rewriteTaskLineForLocalFallback(line, "x", date)`
- which is `replaceTaskStatusSymbol` plus
  `helpers.addOrReplaceCompletionField` when the next symbol is `x`

Already Done and Cancelled cases are recorded unchanged (the verdict is
`already_closed`; the cycler is not asked to rewrite). Recurring cases
are recorded as refused (`is_recurring_task_line`) without calling the
rewriter.

`rule`, `key`, and `config` are hand-authored from §18. Change them only
when §18 changes.

## Regenerating `done`

Do not hand-edit that section. In a bob-plugins checkout at the commit
above (or a later commit whose local-fallback Done rewrite is still the
authority):

1. Require `scripts/task-status-cycler-harness.cjs`.
2. For each completable `done` case, run
   `rewriteTaskLineForLocalFallback(line, "x", date)` on the **logical**
   line (no trailing CR). A `eol: "crlf"` case pins that the file
   terminator stays `\r\n` around that logical rewrite.
3. Replace the `expected.line` values with the rewriter output.
4. Review every generated postimage against `!` / `complete_task_tree`
   bytes (`  [completion:: YYYY-MM-DD]` before a trailing `^id`). If
   cycler and Rust disagree, stop and record it on the bead rather than
   encoding a surprise.
5. Update this README's bob-plugins commit.

## Section contracts

| Section | Pins |
| --- | --- |
| `done` | source-line preimage plus date → postimage, unchanged closed lines, recurring refusal |
| `rule` | configured cancel and complete labels, label, previous label, action, direction, source state, new-task, modifiers, trigger → `verdict`, `effect`, and `refusal` |
| `key` | `relation_label_key` matching against configured complete labels |
| `config` | YAML text → effective cancel keys, complete keys, and warning count |

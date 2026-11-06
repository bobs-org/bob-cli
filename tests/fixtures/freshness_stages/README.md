# Checklist stage conformance vectors (SC + CS)

Shared fixture for `docs/freshness.md` §10 "Checklist stages
(SC1–SC10, CS1–CS16)". The Rust suite
(`src/native/freshness/stage_tests.rs`) is the source of truth and
runs every case; bob-ledger-tools copies this file byte-for-byte to
`plugins/bob-ledger-tools/scripts/fixtures/freshness_stages/vectors.json`
and runs every case whose `runners` include `js` (see that fixture's
README, which pins this repo's commit).

Shape:

- `defaults`: `today` (2026-11-09), `pre_stages`, and `post_stages`
  for queue cases that do not override them.
- `config`: one entry per SC vector: `id`, `runners`, `yaml` (the
  `freshness:` document handed to the real config parser), the
  expected effective `pre` / `post` lists, the expected `interval`
  when the vector pins it, and `diagnostics` (one `{key}` per
  rejected list, in `pre_stages`, `post_stages` order).
- `queue`: one entry per CS vector: `id`, `runners`, optional
  `pre_stages` / `post_stages` overrides, `rows`, the expected
  `order` (`path:line` queue keys), `stages` (one entry per queued
  row: `{stage, rank, total}` or `null` when unstaged), optional
  `tiers` / `lanes` spot checks, and `lints` in canonical emission
  order (per-row `checklist_stage_conflict` in scan order, then
  `freshness_stages_invalid` in `pre_stages`, `post_stages` order,
  then `checklist_stage_unused` in the same key order).

Rows are neutral on purpose: `path`, 1-based `line`, Tasks `status`
symbol, `#`-prefixed `tags`, optional `scheduled` date, and
`recurring`. Each runner builds its own inputs from those fields:
`is_todo` follows the status symbol (`' '` only), `lane_visible` is
false only for future-scheduled rows in this fixture (no blocked,
hidden, template, daily-note, or Today rows appear), `open` is false
only for `x`/`X`/`-` rows (CS14's done typo row), and `checklist`
kind comes from the same `#gtd` + `#pre`/`#post` whole-token rule as
production. Tag position on the line is not modeled: stage
resolution is position-free by contract, so CS16's trailing
`#browser` is the same tag token anywhere else (the JS label phase
treats adjacency as a display rule only). Cases carrying lint
assertions name their languages in `lint_runners` (default Rust
only); only the config-level lint is mirrored in JS.

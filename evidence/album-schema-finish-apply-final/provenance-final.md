# Album Schema Card final acceptance evidence

Date: 2026-11-13

## Real Obsidian acceptance

- Host: Athena
- Snapshot: `/tmp/album-schema-finish-apply-athena-20261113T153643Z/plugin`
- Snapshot manifest: 1,190 file SHA-256 entries match the local bob-plugins tree;
  see `plugin-snapshot-local.sha256` and `plugin-snapshot-athena.sha256`.
- Node: `/tmp/node-v24.15.0/bin/node`, v24.15.0, SHA-256
  `d1de76d8edf2fededf6f8b30d244e2c0529ac607923a018283b77e9c74bd932c`.
- Bob: `/tmp/album-schema-apply-athena-20261113T142039Z/bob`, v0.5.0, SHA-256
  `48d77f5c20d7950e2825bcdde812eb0900bd4f073a0759cbcaa0b9817a801b3b`.
- Obsidian: `/snap/bin/obsidian`, snap v1.15.1, revision 73.
- Result: `pass: true`, 85 checks, zero failures. The report is
  `album-schema-apply.json`; all 11 screenshots were reviewed and their local and
  Athena SHA-256 manifests match.

## Isolated startup smoke

- Ran on Athena from the same byte-verified snapshot with Node v24.15.0.
- Result: pass; 11 expected plugins loaded; layout ready in 324 ms; peak JS heap
  41.5 MiB; relation index ready with zero non-Markdown keys; four editor cases
  passed; no exceptions, rejections, or crash artifacts.
- Report: `obsidian-startup-smoke.json`.

## Plugin gates

- `npm run build`, `npm run build:check`, and `npm run validate` passed; validation
  reports 11/11 plugins.
- Focused album tests after the merge: 50 passed, zero failed, one expected skip.
- The three load-sensitive checks that failed in the full post-merge `npm test`
  run each passed individually: event-link-times, frees-marks, and off-clock-link
  chips. The full run reported 6,452 passed, 3 failed, and 3 skipped; the failures
  are already tracked as `bob-cli-km`, `bob-cli-io`, and `bob-cli-ki`.
- The exact plan command `BOB_CLI_DIR=<primary checkout> npm run docs:check`
  passed word-budget lint, generated-content checks, strict Zensical build, and all
  published URL checks. Offline lychee reports only the existing `bob-cli-l7`
  relative link in `docs/gcal-notes.md` to `reference/mac-capture/event-notes`.
- Primary `timeout 1800 just check` failed at `bob-obsidian --lib`; the captured
  rerun reported 6,879 passed, 2 failed, and 3 ignored. The gcal alias failure is
  tracked as `bob-cli-l2`; the detached-grandchild output test is tracked as
  `bob-cli-kl` and passed in isolation.

## Guarded deployment status

- Current `bob plugins list --format json --no-pull`: 11 installed, 9 synced, 2
  drifted, 0 missing. Current managed-file hashes are in
  `managed-files-current.sha256`; the JSON and dry-run are saved beside this file.
- The dry-run refuses `bob-ledger-tools` (deployed 2026-11-12 from uncommitted
  workspace 15, source head `3ef045b64ff62bcdb63493d16a2518f311b530b6`) and
  `bob-navigation-hotkeys` (deployed 2026-11-13 from uncommitted workspace 13,
  source head `69c0ce7de19d9bc600712f6fa0ec7aedbde025d3`). Live agent status shows
  `dt--code` on workspace 15 and `dv--code` on workspace 13 still running.
- No force flag or new deployment was used. The post-sync smoke and both bead
  closeouts remain pending until those active deployment owners finish and a safe
  dry-run permits the plan's ordinary `--no-pull` sync.

# Album Schema Card apply acceptance evidence

Date: 2026-11-13

## Real Obsidian acceptance walk

- Host: Athena
- Snapshot: `/tmp/album-schema-apply-athena-20261113T142039Z/plugin`
- Snapshot verification: 1,190 file SHA-256 entries match the local bob-plugins
  checkout exactly; see `plugin-snapshot-local.sha256` and
  `plugin-snapshot-athena.sha256`.
- Node: `/tmp/node-v24.15.0/bin/node`, v24.15.0,
  SHA-256 `d1de76d8edf2fededf6f8b30d244e2c0529ac607923a018283b77e9c74bd932c`.
- Bob: `/tmp/album-schema-apply-athena-20261113T142039Z/bob`, v0.5.0,
  SHA-256 `48d77f5c20d7950e2825bcdde812eb0900bd4f073a0759cbcaa0b9817a801b3b`.
- Obsidian: `/snap/bin/obsidian`, snap v1.15.1 (revision 73).
- Command:

  ```sh
  PATH=/tmp/node-v24.15.0/bin:$PATH \
  BOB_BIN=/tmp/album-schema-apply-athena-20261113T142039Z/bob \
  timeout 900 xvfb-run -a /tmp/node-v24.15.0/bin/node \
    scripts/obsidian-album-ui-e2e.cjs -e /snap/bin/obsidian \
    -r /tmp/album-schema-apply-athena-20261113T142039Z/album-schema-apply.json \
    -o /tmp/album-schema-apply-athena-20261113T142039Z/shots
  ```

- Result: `pass: true`, 85 checks, zero failures. The report is
  `album-schema-apply.json`.
- All 11 screenshots were visually reviewed. The report and image checksums match
  between Athena and this evidence directory (`remote-checksums.sha256` and
  `local-checksums.sha256`).

## Isolated startup smoke

- Ran on Athena from the same SHA-verified snapshot:

  ```sh
  PATH=/tmp/node-v24.15.0/bin:$PATH timeout 900 xvfb-run -a \
    npm run smoke:obsidian -- --report \
    /tmp/album-schema-apply-athena-20261113T142039Z/obsidian-startup-smoke.json
  ```

- Result: `pass: true`; 11 expected plugins loaded; layout ready in 328 ms;
  peak JS heap 41.7 MiB; relation index ready with zero non-Markdown keys; four editor
  cases passed; zero plugin-load errors, exceptions, rejections, or crash artifacts.
- The smoke report records `sourceSha: unknown` because this isolated snapshot omits
  Git metadata; the byte-for-byte snapshot manifest above identifies the tested tree.

## Gates and known unrelated failures

- bob-plugins: `npm run build`, `npm run build:check`, and `npm run validate` passed;
  validation reports 11/11 plugins.
- bob-plugins `npm test`: 6,445 passed, 4 failed, 3 skipped; the separate performance
  suite passed 5/5. The four full-suite load-sensitive checks each passed alone and are
  already tracked by `bob-cli-km`, `bob-cli-io`, `bob-cli-ki`, and `bob-cli-ju`:
  event-link-times, frees-marks, keep-moving-marks-surface, and off-clock-link-chips.
- bob-plugins `docs:check`: strict build and URL checks passed; offline lychee found
  only the already-tracked `docs/gcal-notes.md` link to
  `reference/mac-capture/event-notes`. Corroborating report recorded on `bob-cli-l7`.
- Primary `timeout 1800 just check`: formatting and Clippy passed; the
  `bob-obsidian --lib` target failed. A captured rerun of that target reported 6,879
  passed, 2 failed, and 3 ignored. The failures were:
  - `native::gcal::cli::tests::sync_args_read_last_n_days_aliases_and_default_to_none`
    (`["tomorrow"]` versus `["today", "tomorrow"]`), reproduced in isolation and
    already tracked by `bob-cli-l2`.
  - `native::subprocess_runner::tests::run_bounds_pipe_drain_when_detached_grandchild_holds_output`
    (missing `output truncated`), already tracked by `bob-cli-kl`; the focused run on
    the unchanged checkout passed both raw and regular variants (2/2).
- Corroborating reports were recorded on `bob-cli-l2` and `bob-cli-kl`.

## Guarded deploy status

- Pre-sync `bob plugins list --format json --no-pull` reports 11 installed plugins,
  9 synced, 2 drifted, and none missing. The pre-sync JSON and managed-file hashes are
  saved beside this file.
- The dry run exited 1 and refused `bob-ledger-tools` and
  `bob-navigation-hotkeys`: both deployed builds are stamped as dirty from
  `/home/bryan/.local/state/sase/workspaces/bobs-org/bob-cli/bob-cli_15/sase/repos/linked/bob-plugins`,
  source head `3ef045b64ff62bcdb63493d16a2518f311b530b6`, host Apollo, deployed
  2026-11-12 22:58:03 UTC. `origin/master` was fetched; it contains no reachable commit
  whose managed bytes match those stamps. No SASE agent for that workspace or the
  refusal's author is running.
- Deployed manifest versions are bob-ledger-tools 1.113.0 and
  bob-navigation-hotkeys 2.81.0. Their stamped `main.js` hashes are
  `ca35d0797dd27f3c200609129fcff97309f25469a9e5cffd6272bbf8348ef0a0` and
  `dd669807118510263614ea027d53db5309504bf190b96b6120f92fac29062a59` respectively.
- No real sync was attempted and no `-F` was used. The mandatory deployed-copy Athena
  smoke remains pending a decision about those two uncommitted vault builds.
- Exact dry-run refusal: `deploy-dry-run.txt`.

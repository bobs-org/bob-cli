# Attachment origins

Every image or file bob copies into the vault remembers where it came
from. The `Z` image viewer copies that path back out (`y`) or turns the
link into it (`r`), and `Z` on a bullet holding only an image path
imports it through the same copy rules. One loop: path → embed → path.

This file is the contract every implementation cites. The Rust side is
`src/native/attachments/` (origin journal, resolution, and both
commands) plus the `%c` recording in `src/native/capture/` and the
shared planner in `src/native/capture_clip/`. The JavaScript twins are
the `attachments` api namespace in bob-ledger-tools and the origin
line, chips, pill, and key actions in bob-navigation-hotkeys. If an
implementation disagrees with this file, fix this doc first in the same
change.

## Origin journal

The journal is `.bob/attachments/<host>.jsonl`, written through
`pref_journal` with the `ATTACHMENTS` namespace (dir
`.bob/attachments`, noun `attachment`). It records origins rather than
preferences, but follows the shared journal rules: each host appends to
its own file, readers union every `*.jsonl` in the directory and never
rewrite them, and bob is the only writer. One event per copied
attachment:

```json
{"v":1,"ts":"2026-11-03T14:28:40-05:00","host":"apollo","kind":"attachment.import","saved":"img/20261103_142837.png","source":"/Users/bryan/tmp/screenshots/20261103_142837.png","display":"~/tmp/screenshots/20261103_142837.png","sha256":"<64 hex>","via":"capture"}
```

- `source`: the absolute path `%c` resolved (it handles quotes,
  `file://`, and `~/`), or the canonical Gmail thread URL when
  `via: "gmail"`.
- `display`: `source` with the writer's home-directory prefix written
  as `~` for local paths (otherwise identical to `source`).
- `sha256`: hash of the copied bytes.
- `via`: `capture` for `%c`, otherwise the import command's `--via`
  value (default `cli`; the viewer passes `zoom`). Gmail imports use
  `gmail` and record a Gmail thread URL as an external origin.

Reused destinations still get an event, because the new source is still
this embed's origin. Snippet files (`file/clip-*.md`) get none, because
they are not attachments.

## Resolution

`bob attachments origin <VAULT_PATH>` resolves a vault-relative path
P. If P does not exist, it fails with `not_found`.

1. **Candidates.** First, events whose `sha256` equals P's current
   SHA-256; this survives Obsidian renames and dedupe. If there are
   none, events whose `saved` equals P; this covers a vault copy edited
   after import. If there are none, the result is `origin: null`.
2. **Pick.** Take the newest candidate in writer order that was recorded
   on this host and whose source is present. If there is none, take the
   newest candidate.
3. **State.**
   - Recorded on another host → `elsewhere` (never probed).
   - Source missing → `missing`.
   - Source bytes differ from the event's `sha256` → `changed`.
   - Otherwise → `present`.
   - Gmail thread URL → `external` (the URL is never probed as a local path).

## `bob attachments`

The group lives in the Vault section. Bare `bob attachments` prints
help: it has no flag-only read-only member. Every public long option
gets a short alias, and options are sorted alphabetically.

- **`bob attachments import [-b DIR] [-f human|json] <PATH>...`** (plus
  the hidden internal `--via CLIENT`).
  - Takes 1–10 paths. Each is classified exactly as a `%c` clipboard
    line is (`classify_path_candidate`) and copied with the `%c`
    planner: `img/` vs `file/`, the name sanitizer, reuse of an
    identical file, and the 8-character hash suffix on a conflict. It
    never edits notes.
  - Runs under `bob_sync.lock` with a 3 s wait (`ob::acquire_lock_waiting`;
    busy gives `lock_busy`). It appends journal events after the files
    are saved. If the append fails, it removes the files it created
    (reuse `capture_clip::cleanup_created`) and fails.
  - JSON output:
    `{"schema_version":1,"attachments":[{"source","display","saved","kind","reused","embed","sha256"}]}`.
    `embed` is `attachment_reference` output (`![[img/x.png|400]]` /
    `[[file/x.pdf]]`).
  - Failures reuse the `bob relations hide -f json` error envelope,
    with codes `missing`, `not_a_file`, `not_absolute`, `too_many`,
    `lock_busy`, and `io`.
  - Exit codes: 0 ok, 1 failure, 2 usage.
  - Human output is one colored line per file:
    `imported  ~/tmp/…/x.png → img/x.png` or
    `reused    ~/tmp/…/x.png → img/x.png`.
- **`bob attachments origin [-b DIR] [-f human|json] <VAULT_PATH>`** is
  read-only.
  - JSON output:
    `{"schema_version":1,"path":"img/x.png","origin":{"source","display","host","ts","via","state"}|null,"warnings":[]}`.
  - Human output:
    `img/x.png ← ~/tmp/…/x.png  present · <host> · 2026-11-03 14:28 · capture`,
    or `img/x.png has no recorded origin`.
  - Exit codes: 0 resolved (including `null`), 1 `not_found` or I/O,
    2 usage.

## `%c` recording

After a non-dry-run capture commits, bob appends one event per committed
attachment (`via: "capture"`, history entries included), next to the
relation-label hide appends. A failed append becomes a capture warning
(`could not record attachment origin: …`) and never fails the capture.
`--dry-run` appends nothing. The capture JSON is unchanged: the new
`sha256` field never serializes.

## Vault copies are never deleted

Neither `import`, `origin`, `%c` recording, nor the viewer's `r` restore
ever deletes a vault copy: other notes may embed the same file, so the
vault copy stays and only links change.

## Related

- [Clipboard capture](capture.md#clipboard)
- [Bob vault Git sync runbook](vault-git-sync.md#attachment-origin-journal)
- [JSON outputs](reference/json-outputs.md)

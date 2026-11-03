# Event buffer vectors

`vectors.json` is the shared Rust/Obsidian contract for description
directive lines, the `[buffer::]` vault field, buffered spans, and the
conflict predicate. The Rust tests consume it in bob-cli
(`src/native/gcal/description.rs` and `src/native/event_task.rs`). The
Obsidian mirror lives at
`plugins/bob-ledger-tools/scripts/fixtures/event_buffer/vectors.json`
in bob-plugins and must copy this file from the bob-cli commit that
first adds it.

Three sections:

- `description`: raw Google description (including HTML) → `route`,
  `untracked`, `before`, `after`, and optionally its explicit `clock`
  (`none`, `off`, or `on`; a `#on`/off-tag conflict resolves to `on`).
- `row`: vault Event Task line → `buffer`, `buffered_span`, clean
  `title`, canonical `field`, and human `label`.
- `conflict`: session `new`/`old` spans plus a day's Events → crossed
  block id, `overlap_minutes`, `buffer_part`, and `fit`.

The vectors use only description strings and hand-written Event Tasks;
no Google Calendar account or generated sync state is needed.

Google clock directives use canonical `#off` and `#on`. The deprecated
`#offclock`, `#off-clock`, and `#untracked` spellings remain input aliases.
The Rust consumer checks `clock` only when a case provides it, keeping the
existing shared cases stable for the plugin harness.

Buffer-reason lines (`-30 Drive to Kelly's`) live in bob-cli-only
`tests/fixtures/gcal_description/outline.json`, so this shared file does
not grow: no existing case starts with a buffer token followed by a word.

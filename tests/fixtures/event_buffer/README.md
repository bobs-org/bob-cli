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
  `untracked`, `before`, `after`.
- `row`: vault Event Task line → `buffer`, `buffered_span`, clean
  `title`, canonical `field`, and human `label`.
- `conflict`: session `new`/`old` spans plus a day's Events → crossed
  block id, `overlap_minutes`, `buffer_part`, and `fit`.

The vectors use only description strings and hand-written Event Tasks;
no Google Calendar account or generated sync state is needed.

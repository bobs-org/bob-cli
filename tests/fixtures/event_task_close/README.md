# Event Task close vectors

`vectors.json` is the shared Rust/Obsidian clock-close contract. The Rust tests
consume it in bob-cli. The Obsidian mirror lives at
`plugins/task-status-cycler/scripts/fixtures/event_task_close/vectors.json` in
bob-plugins and must copy this file from the bob-cli commit that first adds it.

Pin: `buffered-event-classifies-by-its-meeting-span` was added with the
`[buffer::]` field in bob-cli. A buffer never widens a close: the clock class
comes from the bare meeting span, so a MEET closed when lunch ends still
completes lunch even though its `+15` buffer runs on. A bob-plugins copy that
predates this case is stale: re-copy this file from the bob-cli commit that
adds it.

The vectors use only hand-written Event Tasks; no Google Calendar account or
generated sync state is needed.

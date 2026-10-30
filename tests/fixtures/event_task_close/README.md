# Event Task close vectors

`vectors.json` is the shared Rust/Obsidian clock-close contract. The Rust tests
consume it in bob-cli. The Obsidian mirror lives at
`plugins/task-status-cycler/scripts/fixtures/event_task_close/vectors.json` in
bob-plugins and must copy this file from the bob-cli commit that first adds it.

The vectors use only hand-written Event Tasks; no Google Calendar account or
generated sync state is needed.

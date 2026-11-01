# MEET event-run vectors

`vectors.json` is the shared Rust/Obsidian contract for how a MEET start follows
its event run and which sessions are tracked. The Rust test
(`tests/cli/capture/meet_event_run.rs`) runs every case through
`bob capture -f json`. The Obsidian mirror lives at
`plugins/bob-ledger-tools/scripts/fixtures/meet_event_run/vectors.json` in
bob-plugins and must copy this file from the bob-cli commit that first adds it.
bob-plugins drives each `=`, `=meet`, and `=5#meet` case as an `se<X>` + Tab
inside the target `()` (a restart replaces the running range with `(se)`); the
`~K` drop and kept-ledger swap cases are capture-only gestures.

Each case gives the whole daily note (`day`, path `2026/20261101.md`), the
clock (`now`), the capture `token`, and the expected note after the start
(`expected_day`), whether the written session is tracked (`expected_tracked`,
false means the ledger has no `[t::]`), and the written span (`expected_span`).
`expected_source` is the `pomodoro_start.event_run.source` the JSON reports
(`calendar` when the span follows the run, `explicit` when a counted suffix or a
kept ledger set it, absent when the MEET has no run). `expected_warning` is a
structured warning `code` the start raises.

The vectors use only hand-written Event Tasks; no Google Calendar account or
generated sync state is needed.

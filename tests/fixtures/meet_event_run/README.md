# MEET event-run vectors

`vectors.json` is the shared Rust/Obsidian contract for how a MEET start follows
its event run and which sessions are off-clock. A non-MEET start whose counted
lineup is all `#offclock` (or an alias) writes `[off::]` too; only a MEET with
a non-empty event run classifies from the run instead of the whole lineup. The Rust test
(`tests/cli/capture/meet_event_run.rs`) runs every case through
`bob capture -f json`. The Obsidian mirror lives at
`plugins/bob-ledger-tools/scripts/fixtures/meet_event_run/vectors.json` in
bob-plugins and must copy this file from the bob-cli commit that first adds it.
bob-plugins drives each `=`, `=meet`, and `=5#meet` case as an `se<X>` + Tab
inside the target `()` (a restart replaces the running range with `(se)`); the
`~K` drop and kept-ledger swap cases are capture-only gestures.

Each case gives the whole daily note (`day`, path `2026/20261101.md`),
optional other notes (`notes`, path → contents; notes may carry
frontmatter), the clock (`now`), the capture `token`, and the expected note
after the start (`expected_day`), whether the written session is off-clock
(`expected_off_clock`), an optional shared inherited note path
(`expected_clock_note`), and the written span (`expected_span`). Off-clock
spans carry `[off:: Nm]`; worked spans carry `[t:: Nm]`.
`expected_source` is the `pomodoro_start.event_run.source` the JSON reports
(`calendar` when the span follows the run, `explicit` when a counted suffix or a
kept ledger set it, absent when the MEET has no run). `expected_warning` is a
structured warning `code` the start raises.

Pin: copy this file from the bob-cli commit that adds the inherited-root
frontmatter case. Earlier buffered cases (ids containing `buffer` or
`lead-out`) remain; a bob-plugins copy that predates the inherited-root
case is stale. A buffered event runs on its buffered span
(`start - before` through `end + after`): candidates, sorting, clustering,
the written span, the early start, and the far-run warning all use it,
while a run never bridges trackedness.

The vectors use only hand-written Event Tasks; no Google Calendar account or
generated sync state is needed.

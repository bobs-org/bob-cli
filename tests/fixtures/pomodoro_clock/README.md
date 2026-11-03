# Pomodoro clock vectors

`vectors.json` is the shared Rust/Obsidian contract for the counted-links
clock rule: a session is off-clock iff it keeps at least one Task Link and
every kept link resolves off-clock. `#off` is the canonical off-clock tag
(`#offclock`, `#off-clock`, and `#untracked` stay aliases); a task can also
inherit `offclock: true` from the nearest area or project on its root walk,
with `#on` and `offclock: false` as overrides. Mixed, unresolved, and
unlinked lineups stay on the work clock. The Rust test
(`tests/cli/capture/pomodoro_clock.rs`) runs every case through
`bob capture -f json`. The Obsidian mirror lives at
`plugins/bob-ledger-tools/scripts/fixtures/pomodoro_clock/` in bob-plugins
and must copy this file from the bob-cli commit that first adds it.
task-status-cycler drives the close cases.

Each case gives the whole daily note (`day`, path `2026/20261101.md`),
optional other notes (`notes`, path → contents; notes may carry
frontmatter), the clock (`now`), the capture `token`, the expected note
after the write (`expected_day`), whether the written session is off-clock
(`expected_off_clock`), an optional shared inherited note path
(`expected_clock_note`), and an optional structured warning `code`
(`expected_warning`). Off-clock spans carry `[off:: Nm]`; worked spans
carry `[t:: Nm]`. A MEET with a non-empty event run uses that run's clock;
future lineup events are queue and never mix the run. Close cases stamp the
closed row from the links it keeps after by-the-clock classification and
the selection.

Pin: copy this file from the bob-cli commit that adds the frontmatter
cases. A bob-plugins copy that predates those cases is stale.

# Plan budget off-clock vectors

`vectors.json` is the shared Rust/Obsidian contract for the
off-clock-links-are-uncapped rule: a Task Link whose target task resolves
off-clock never counts toward `plan.max_links`, never makes the status
`over`, never raises `plan_link_cap_exceeded`, and never fires capture's
link-cap warning. Off-clock tasks stay Today tasks. The Rust test
(`tests/cli/plan_off_clock.rs`) runs every case through
`bob plan -f json` in a temp vault built like the existing plan tests
(Tasks `data.json`, `BOB_NOW=2026-11-05 12:00:00`, `max_links` through the
test config). The Obsidian mirror lives at
`plugins/bob-ledger-tools/scripts/fixtures/plan_budget_off_clock/` in
bob-plugins and must be copied byte-for-byte from the bob-cli commit that
adds it.

The rule, also in `docs/plan.md` §Links:

- The budget finds candidate links by syntax alone: distinct
  `(target, block_id)` pairs under open, non-exempt entries, with `.md`
  stripped and daily-note aliases canonicalized.
- Each distinct pair is then classified once with the same link-clock
  resolution the session clock rule uses
  (`docs/capture.md` counted-links rule /
  `decisions:off-clock-follows-tag-then-note-default`). Same-note links
  (the empty target or the canonical daily key) check the daily note's
  Event Tasks first, then any task with that block ID. Other links resolve
  the target note and then the single task line with that block ID. The
  task's effective clock is decided by `#on`, then `#off` (or a deprecated
  alias), then the nearest area/project `offclock:` on its root walk
  (including `[root::]`), and otherwise on-clock.
- A pair whose task resolves off-clock is an _off-clock link_. It is left
  out of `links.count`, out of the entry's `links`, out of `status`, and
  out of `plan_link_cap_exceeded`. It is reported in a new additive meter
  field `links.off_clock` (JSON `off_clock`, JS `offClock`): the number of
  distinct off-clock pairs.
- Unresolved, ambiguous, unreadable, or not-yet-cached targets stay
  counted.
- Task status does not matter: a done or cancelled off-clock task's link
  was never counted for its status before, and it is still off-clock now.
- A mixed session (one on-clock link and one off-clock link) still runs on
  the work clock as a session. The budget, however, counts only its
  on-clock link.
- Unchanged: types, the `(unnamed)` type rule, and empty-placeholder
  skipping still look at raw links (off-clock ones included), and exempt
  entries still contribute nothing.

Case schema: `id`, `day` (contents of `2026/20261105.md`), optional `notes`
(path → contents, frontmatter allowed), optional `max_links`, and the
expected `links_count`, `links_off_clock`, `status`, and `entry_links`
(ordered per listed entry), plus a Rust-only `today_count`.

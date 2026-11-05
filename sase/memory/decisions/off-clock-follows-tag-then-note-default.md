---
keyword: Off-Clock Follows Tag Then Note Default
aliases:
- offclock note default
- '#off #on tags'
- inherited off-clock
summary: 'A task''s clock is #on, then #off (or a deprecated alias), then the nearest
  area/project offclock default on its root walk, then on-clock; writers stamp that
  effective clock and closed rows stay frozen.'
metadata:
  status: superseded-in-part
  decided: 2026-11-03
  superseded_by: decisions:off-clock-note-defaults-filter-by-task-type
---

# Off-Clock Follows Tag Then Note Default

**Applies to.** bob-cli capture writers, bob-plugins starts and closes, Google
Calendar clock tags, and the vault.

**Claim.** A task's clock is the first match: a standalone `#on` on its own
line (wins a conflict with an off spelling); else `#off` or a deprecated
alias (`#offclock`, `#off-clock`, `#untracked`); else the nearest area or
project `offclock: true|false` on the task's PT13 root walk; else on-clock.
An invalid nearer `offclock:` value is on-clock. Writers resolve that
effective clock when they stamp `[t::]` / `[off::]`; closed rows stay frozen,
and toggling a note default later surfaces as drift. `#off` is the only off
spelling any writer emits. Alias deprecation warnings appear only at the
Google boundary (`bob gcal sync`, `bob gcal list`). The counted-links rule is
unchanged except that "off-clock" now means the effective clock, so a love
task mixed with a job task stays Worked. Ended, unstarted Event Tasks that
inherit off-clock settle like tagged ones.

**Why.** Off-clock life already lives under `love.md` and its sub-areas; a
note default removes the per-task tag without materializing `#off` onto every
row. `#off` matches the `[off::]` key it produces. Stamp-time evaluation keeps
closed history honest.

Rejected alternatives:

- **Materialize inherited `#off` / `#on` onto task lines.** Sync would fight
  Google titles, and toggling a note default would rewrite history.
- **A Stats-only filter.** The ledger key would still need a tag on every
  lunch and pickup.
- **`clock:` or `task_type: off` as the switch.** The vocabulary is settled:
  `[off::]`, JSON `off_clock`, and the Off-clock Stats lens already exist.
- **Keep `#offclock` canonical.** It is long in a phone Calendar title.
- **Split mixed sessions across clocks.** The session is one timed event.

**Cost.** Mixed lineups stay Worked in full. An invalid nearer `offclock:`
fails toward Worked and does not inherit a parent's `true`. Untagged
love-routed events settle even when nobody attended.

**Reopens when.** Per-task minute accounting is needed, or inherited tags
should be written onto task lines.

Evidence: `plan:202611/off_on_tags_and_offclock_areas.md`;
`research:202611/off_on_tags_and_offclock_areas/off_on_tags_and_offclock_areas.md`.
See [[decisions:off-clock-pomodoros-count-apart]],
[[glossary:off-clock-pomodoro]], [[glossary:event-task]],
[[glossary:area-note]], and [[glossary:project-note]].

[[decisions:off-clock-note-defaults-filter-by-task-type]] retires the note-default rule in part: `offclock_types` on the nearest area or project note decides by effective task type and overrides a same-note `offclock:`.

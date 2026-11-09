---
origin: requested
scope: project
summary: "`bob gcal sync -F` explicitly resets one day's sync journal and restores missing rows from Google while keeping task and Pomodoro history."
aliases:
  - gcal sync -F
  - gcal day reset
---
# A Forced Sync Resets The Day

**Applies to.** bob-cli and the vault.

**Claim.** A journaled row missing from the note is treated as your deletion
and tombstoned. When every row Bob wrote for the day and the Events stamp
vanish together, sync holds instead of tombstoning them. Only an explicit
`bob gcal sync -F` restores missing rows from Google: it resets that day's
sync journal and rewrites the day as on a first sync. A row Bryan pastes back
remains adoptable as crash recovery. For rows still present, Google replaces
the time, title, badges, and chosen root; the checkbox, block ID, child notes,
`[type::]`, Pomodoro sessions, and learned series roots stay. Existing
Pomodoro links to reused block IDs are not duplicated, and a removed link is
filed again. The accepted `force_edits = reset` branch applies.

**Why.** The 2026-11-09 Haircut race report showed sync writing an Event Task
before Templater replaced the raw daily note, then recording the missing row
as a permanent deletion. Auto-restoring every missing row would re-add rows
without consent; a permanent tombstone with only hand-paste recovery leaves a
clobbered day stuck; and sticky tombstones make explicit recovery impossible.
`-F` makes recovery explicit while rebuilding the day from the authoritative
Google snapshot.

**Cost.** A held day gets no new events until `-F`. A forced reset also
disregards edits to the Google-owned spans and `[root::]` on present rows,
while retaining task state, notes, Pomodoro history, and series memory.

**Reopens when.** Bob can tell an editor clobber from a deliberate deletion
directly.

Evidence: `research:202611/gcal_haircut_lost_to_daily_note_race.md`,
`plan:202611/gcal_sync_race_force_and_skip_reasons.md`, `docs/gcal.md` § Sync
› Reset a day, and `tests/gcal_sync.rs`.

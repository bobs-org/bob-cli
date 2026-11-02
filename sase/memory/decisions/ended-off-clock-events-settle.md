---
keyword: Ended Off-Clock Events Settle Into Closed Sessions
aliases:
  - settle ended off-clock events
  - off-clock event settlement
summary: "Reconcile settles ended off-clock Event Tasks left under an unstarted placeholder into closed sessions and completes their task rows."
metadata:
  status: accepted
  decided: 2026-11-02
---

# Ended Off-Clock Events Settle Into Closed Sessions

**Applies to.** bob-cli and the vault.

**Claim.** `bob task reconcile` settles each ended, open off-clock Event Task
still linked under an unstarted placeholder into a closed `[off:: Nm]` MEET,
trimmed so it does not overlap an on-clock session. The Event Task is completed
with `[x]` and its completion date, using the same completion behavior as a
timed close for a meeting that ended inside the session. The operation is
idempotent.

**Why.** The off-clock clock needs the time even when nobody starts the MEET.
Reconcile already runs periodically and can record it without asking for a
separate gesture. Completing the row matches a timed close that covers an
ended meeting; leaving it `[/]` would keep a lunch in Pending.

Rejected alternatives:

- **Settle in `bob gcal sync`.** Sync is on demand and usually does not run
  after an event ends.
- **Settle at capture time.** That requires another user gesture after the
  event and misses events nobody starts.
- **Do not settle.** An ended off-clock event nobody started would never be
  recorded on the Off-clock clock.
- **Leave the event `[/]`.** The selected completed status avoids parking
  settled events in Pending.

**Cost.** The calendar is not proof of attendance; a skipped event is counted
unless it is cancelled before it ends. The task row is completed during
settlement.

**Reopens when.** Attendance becomes knowable, or settling should also cover
running sessions.

Evidence: `plan:202611/off_clock_pomodoros.md`; `docs/task-status-hooks.md`;
`docs/gcal.md` § Off-clock events.

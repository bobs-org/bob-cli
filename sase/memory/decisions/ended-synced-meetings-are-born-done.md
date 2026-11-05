---
keyword: ended-synced-meetings-are-born-done
aliases:
  - past meetings born done
summary: Event Tasks first synced after their buffered span ends are born `[x]` with the note-day completion date and a struck 🍅 link under their closed MEET.
metadata:
  status: superseded-in-part
  superseded_by:
    - decisions:meet-filing-follows-google-until-edited
  decided: 2026-11-04
title: Ended Synced Meetings Are Born Done
---

# Ended Synced Meetings Are Born Done

**Applies to.** bob-cli and the vault.

**Claim.** Whenever `bob gcal sync` creates an Event Task for a Past-phase
meeting — a new row or a Maybe context row upgraded in place, on any note day
— it writes `[x]` with `[completion:: YYYY-MM-DD]` for the note's day, then
files a struck `🍅 ~~[[#^id]]~~` link under the closed MEET. Current and
Future rows stay `[*]`, and sync never changes an existing row's checkbox.

**Why.** Bryan's 2026-11-04 request follows the `^swings` row in
`2026/20261104.md`: synced at 11:06 for an 08:15–08:30 event, it stayed `[/]`
in an already over-cap Pending lane even though its closed MEET was correct.
Completing the row follows reconcile's completion of ended off-clock events.

Rejected alternatives:

- **Keep `[/]`.** It parks finished meetings in Pending.
- **Use the sync day as completion date.** It miscounts `-D yesterday` rows.
- **Leave the 🍅 link plain.** Reconcile rewrites it after sync.

**Cost.** A skipped meeting is born done and counted; uncheck or cancel it by
hand.

**Reopens when.** Sync can learn attendance, or skipped meetings should stay
open for review.

Evidence: `plan:202611/gcal_past_events_born_done.md`; `docs/gcal.md` § Sync;
`tests/gcal_sync.rs`.

[[decisions:synced-meetings-link-by-phase]]

[[decisions:ended-off-clock-events-settle]]

[[glossary:event-task]]

Superseded in part: a born-done Event Task moved into Current or Future can be reopened and re-filed; the rule that sync never changes an existing row checkbox is retired for that move. See [[decisions:meet-filing-follows-google-until-edited]].

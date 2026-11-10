---
keyword: Private Event Notes Beat Google Per Field
aliases:
  - private-event-notes-beat-google-per-field
  - private event notes
  - private event note
summary:
  "A private event note is vault-stored Bob-only description text keyed by
  the Google event; it beats Google per field, and the row keeps one
  ownership rule."
metadata:
  status: accepted
  decided: 2026-11-10
---

# Private Event Notes Beat Google Per Field

**Applies to.** bob-cli, Bob Mac Capture, and the vault.

**Claim.** A private event note is vault-stored, Bob-only description text
keyed by the Google event. It beats Google's description per field: this
event, then all events in the series, then Google; a reason travels with
its side's winning value. The Event Task keeps one ownership rule: it
follows the layered description while untouched, your edit wins, and that
edit is reported once. Google stays read-only. The note never changes
inclusion, never RSVPs, and never creates a row.

**Why.** Bryan needs `@route`, `#off`/`#on`, buffer reasons, and prose on
events he does not own, including invitations he has not answered yet, at
RSVP time — weeks before the daily note (and so the Event Task) exists.
The same description grammar already writes those fields on events he can
edit. Layering a journaled note over Google's description keeps that
grammar, keeps [[decisions:bob-gcal-is-read-only-and-proposes-once]], and
keeps the row's one follow-until-edited rule from
[[decisions:gcal-root-follows-google-until-edited]] and
[[decisions:buffer-reasons-ride-description-bullets]]. A note written after
the row exists behaves like a Google-side description change.

Rejected alternatives:

- **Writing the lines to Google or to `extendedProperties`.** The grant is
  read-only, and Google would show Bob's private logistics to guests.
- **The RSVP comment.** It is public, not structured, and Bob cannot write
  it.
- **Pre-touched overrides.** A vault edit before the row exists has no row
  to touch, and it would invent a second ownership rule.
- **Seed-once.** The note could not change after birth, so an RSVP-time
  note could not be corrected before the day.
- **A separate ownership block.** Two adjacent windows and two journal
  records for one description.
- **Shadow events.** Duplicate Google events that fight the real invite.
- **Pre-created daily notes.** Daily notes are born on their day; the
  design never pre-creates them.
- **Markdown holding notes.** A note file per event splits identity from
  Google's event id and is not the gcal journal.
- **A capture sigil for the Mac surface.** Capture grammar is for tasks;
  Event note mode is a dedicated surface that calls `bob gcal note`.

**Cost.** The badges span is one hash, so a note changed after the row's
birth reaches the buffer and clock tag only while the time, title, and
badges are all untouched. Older binaries print one skip warning per
`gcal.note` journal line until upgraded. Bob occupancy from a note is not
Google free/busy.

**Reopens when.** RSVP-in-Bob is wanted, reasons must survive hand-edited
blocks, or notes must reach rows with edited titles.

Evidence: `plan:202611/private_gcal_event_notes.md`;
`research:202611/private_notes_for_unowned_gcal_events/private_notes_for_unowned_gcal_events.md`;
`docs/gcal-notes.md`; `tests/gcal_notes.rs`.

[[decisions:bob-gcal-is-read-only-and-proposes-once]]

[[decisions:gcal-root-follows-google-until-edited]]

[[decisions:event-buffers-widen-the-span]]

[[decisions:buffer-reasons-ride-description-bullets]]

[[glossary:event-task]]

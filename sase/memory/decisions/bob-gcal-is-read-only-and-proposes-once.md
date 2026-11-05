---
origin: requested
scope: project
aliases:
  - gcal propose-once
  - read-only gcal
summary: Google Calendar stays read-only; Bob writes each row and link once, updates only untouched Google spans, tombstones deletions, and cancels only untouched tasks.
metadata:
  status: superseded-in-part
  superseded_by:
    - decisions:gcal-root-follows-google-until-edited
    - decisions:meet-filing-follows-google-until-edited
---

# Bob Gcal Is Read-Only And Proposes Once

**Applies to.** bob-cli and the vault.

**Claim.** The Google Calendar grant is read-only, and Bob writes each row
and link once: Google-owned spans (time, title, badges) update in place
only while the row is untouched; the vault owns the checkbox, `[root::]`,
children, and the row itself; deleted rows are tombstoned and never
re-added; only untouched tasks are cancelled (declined, deleted, or moved
events); an incomplete snapshot (a failed page or lookup) writes nothing;
and the Mac is the single writer.

**Why.** Two-way sync would fight Bryan's edits to titles, roots, and
rows, and iCal URLs or Obsidian calendar plugins would duplicate the
daily-note residence (see
[[decisions:calendar-events-are-daily-note-event-tasks]]). Rejected:
two-way sync (edit conflicts with no UI to resolve them); iCal URLs
(no RSVP or root precedence); and Obsidian calendar plugins (they render
outside `## Events` and the MEET ledger).

**Cost.** Google-side edits to touched rows are reported, not applied
(`kept_edit`, once per row); missed meetings stay open and surface in
`list` instead of auto-cancelling.

**Reopens when.** Bob needs to RSVP to or create events in Google.

Evidence: `plan:202610/bob_gcal_daily_events.md`,
`docs/gcal.md` § Reconciling Google-side changes, and
`tests/gcal_lifecycle.rs`.

Superseded in part: the `[root::]` ownership clause is retired; an Event
Task's `[root::]` follows Google's chosen root until you edit it. See
[[decisions:gcal-root-follows-google-until-edited]].

Superseded in part: the `children` ownership clause is retired for imported
description bullets. They follow Google while they match what Bob last wrote;
your edit, nested note, or deletion makes them yours, with one notice only if
Google later changes them. Other child notes remain yours. Evidence:
`plan:202611/gcal_description_bullets.md`, `docs/gcal.md` § Description bullets
and § Who owns each part of a row, and `tests/gcal_lifecycle.rs`.

Superseded in part: a qualifying Google time or buffer move can re-file Bob-owned MEET links; the rule that each link is written once is retired. See [[decisions:meet-filing-follows-google-until-edited]].

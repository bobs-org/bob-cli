---
origin: requested
scope: project
aliases:
  - gcal propose-once
  - read-only gcal
summary: Google Calendar stays read-only; Bob writes each row and link once, updates only untouched Google spans, tombstones deletions, and cancels only untouched tasks.
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

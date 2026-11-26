---
title: Existing-Task Links Carry Note Bullets
summary: Non-close existing-task links can also file authored child bullets as notes.
---

aka existing-task link notes, link-with-notes, pomodoro link notes

**Applies to.** bob-cli and Bob Mac Capture.

**Claim.** A note-less non-close existing-task link (`@route:id` or
`^route:id`), including named Pomodoro and link-start forms, may be followed
by authored child bullets. Bob links first, then files each first-level bullet
with its nested children exactly as a note-less `@route+id` does. Drawer
shortcuts and requirement bullets keep that shared multi-note grammar. Link
close forms (`@route:id=x…`, `^route:id=x…`) do not take child bullets as
notes.

**Why.** Bryan's `^sub-bullet-and-today` task in `~/bob/bob.md` documents the
gap: he typed `@bob:h2s-to-area-prj-card` with a `- foo` child, hit the
existing refusal, and had to write the task twice to link it and add its note.
The link-only and note-only spellings remain useful and unchanged.

Rejected alternatives were treating a same-line note as a note on the linked
task (it already creates a new task) and adding a new sigil for the combined
gesture.

**Cost.** Capture grammar revision 34 admits authored children under a
note-less existing-task link. `pomodoro_link` results can now include the
existing `notes` and `drawers` fields.

**Reopens when.** Bryan wants notes on link-close or snooze forms.

**Evidence.** `plan:202611/link_with_note_bullets.md`; Bob task
`~/bob/bob.md#^sub-bullet-and-today`; the bob-cli implementation commit for
grammar revision 34.

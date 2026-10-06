---
keyword: A Task Move Never Advances The Walk
aliases:
  - ctrl+shift+m stays out of the walk
  - move follows the task
  - move parks the walk
summary:
  "Ctrl+Shift+M never advances the ]s walk, even from a landed row: it follows
  the moved task to its destination note and parks the walk, so the next ]s /
  [s resumes at the moved row's walk neighbour."
metadata:
  status: accepted
  decided: 2026-10-06
---

**Applies to.** bob-plugins (bob-navigation-hotkeys), bob-cli docs, and the Bob
vault review ritual.

**Claim.** Ctrl+Shift+M never advances the `]s` walk, even from a landed row:
it follows the moved task to its destination note and parks the walk, so the
next `]s` / `[s` resumes at the moved row's walk neighbour.

- The resume identifies rows by note path and line text, so the line shift a
  move causes never skips a row.
- The gesture lock still swallows Ctrl+Shift+M while another answer settles.
- Pomodoro bullet and entry moves were never part of the walk.
- Every other answer in [[decisions/answering-advances-the-walk]] keeps
  advancing.

**Why.** Bryan keeps working on the task where it now lives; after moving it
he often adds context, dependencies, or a link in the destination note (his
answer in this plan's questions round).

Rejected alternatives:

- **Advancing instead of focusing (the epic `bob-cli-4l` rule, 2026-10-06).**
  It took Bryan away from the note where the follow-up work happens.
- **Staying in the source note without advancing.** The follow-up happens in
  the destination, and the seam is usually the next due row, which a bare
  `]s` from a due row steps past.
- **Restoring the pre-epic move unchanged.** Line-keyed anchors make the next
  `]s` skip the row below the moved task once the Tasks cache refreshes.
- **A per-gesture toggle or config switch.** That is one more setting for one
  key.

Evidence:
`plan:202610/ctrl_shift_m_never_advances_walk.md`;
bob-navigation-hotkeys 2.7.0 (this change);
`docs/freshness.md` §§6/13 in bob-cli.

**Cost.** Reaching the next review item after a move takes one `]s`. The
resume lives only until the next landing or stamp, so an Alt+F or Ctrl+Alt+F
elsewhere in between re-anchors the walk there.

**Reopens when.** Bryan finds himself pressing `]s` right after nearly every
move, or asks for the move to advance again.

Links [[decisions/answering-advances-the-walk]] (the walk this move stays out
of) and [[decisions/review-walk-is-tiered]] (the tier order the resume walks).

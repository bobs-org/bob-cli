---
keyword: Awaiting A Gmail Reply Is A Tagged Schedule Hold
aliases:
  - awaiting reply
  - awaiting_reply
  - gmail reply hold
  - reply wait
summary:
  "A watched Gmail send holds its open tasks with exact #awaiting_reply plus
  one scheduled nudge date; a reply or bounce lifts that hold, and an eligible
  Ready row resurfaces through [unblocked::] after the hold date is gone."
metadata:
  status: accepted
  decided: 2026-11-08
---

**Applies to.** bob-cli, bob-plugins, and the vault.

**Claim.** A watched Gmail thread holds its open linking tasks with the
exact `#awaiting_reply` tag and one `[scheduled::]` nudge date: the send's
local calendar date plus configured `gmail.sync.reply_wait_days`. The tag
follows `#task`, or `#prj_is_blocked` when that token is present. A future
nudge stores `[?]`; an already-arrived date keeps the original box. A new
outgoing counted move replaces that scheduled value. A new incoming counted
move or bounce removes the exact tag and then that scheduled field, recovers
`[?]` to `[ ]` only when no other blocker remains, and runs
`freshness::placement::mark_unblocked` **only after obtaining a Ready row
and after removing the hold date**, so a nudge-due Ready lift still writes
`[unblocked:: today]` (UB16–UB18). WRITE skips stay. Between counted
messages the line is Bryan's: polling does not reassert the hold. Reconcile
treats an awaiting row as an ordinary scheduled hold and never reinserts the
tag. Neither operation stamps, restores a pre-hold lane, or invents a wait
checkbox or field.

**Why.** The hold has to block like any other future date, resurface through
the existing tickler writer, and leave Bryan's edits alone until the next
message. Rejected alternatives:

- **A `[w]` waiting checkbox.** A new lane/status; Blocked already covers
  a future date.
- **A separate wait field.** Duplicates `scheduled` and splits Blocked
  derivation.
- **`[wait::]`.** Same split; writers and marks would have to learn a
  second date.
- **Reassert the hold on every poll.** Overwrites Bryan's date and tag
  edits between messages.
- **Auto-Next on reply.** Invents a lane the lift does not own.

Evidence: ancestor `plan:202611/gmail_sync_reply_watch.md`
(`reply_hold_record=yes`); child `plan:202611/gmail_reply_watch.md`;
bob-cli `b4147b0b` (line rules), `86af3d5e` (watch journal),
`9e0bcdd8` (transaction), `457153f0` (polling), `5a63e9a2` (send hook);
`docs/gmail.md` Watch; `docs/freshness.md` §5 and UB16–UB18.

**Cost.** Next and Pending do not survive the hold: a future nudge stores
`[?]`, and lift recovers to derived Ready when no other blocker remains.
Plugin marks must recognize the exact tag. Changing `reply_wait_days`
never rewrites existing holds.

**Reopens when.** Lanes must survive a Gmail hold, or the nudge must live
outside `[scheduled::]`.

[[decisions:unblocked-tasks-resurface-as-ticklers]]

[[decisions:task-lanes-are-sticky]]

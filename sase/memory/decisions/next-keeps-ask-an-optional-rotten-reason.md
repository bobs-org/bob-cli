---
keyword: Next Keeps Ask For An Optional Rotten Log Reason
aliases:
  - Next keep reason
  - Next Rotten Log entry
summary: Alt+F and Ctrl+Alt+F on a Next task ask why it stays; a nonblank reason writes a Rotten Log entry, while blank Enter stamps without logging or changing the keep streak.
requested: project
metadata:
  status: accepted
  decided: 2026-11-01
---

# Next Keeps Ask For An Optional Rotten Log Reason

*Requested · project*

aka Next keep reason, Next Rotten Log entry

**Applies to.** bob-plugins (bob-navigation-hotkeys and bob-ledger-tools),
bob-cli docs, and the Bob vault.

**Claim.** Alt+F or Ctrl+Alt+F on an accepted Next (`[*]`) stamp target asks
"Why keep it in Next?", whether the task is due in the NEXT tier or still
early. A nonblank answer writes `*YYYY-MM-DD* — <reason> · kept in Next`
under the task's `🥀 **ROTTEN LOG**` child in the same transaction as the
freshness stamp. A blank Enter keeps without a log entry; Esc cancels the
stamp. Next keeps never change `[keeps::]` and never count toward the streak.
Pending keeps their optional Work Log summary and never write the Rotten Log.
Counted `N<Alt+F>` and Task Link sessions use one prompt: Work summary first,
then Keep reason; each answer is routed to its own targets and log. A blank
Next reason writes no entry. The Rotten Log count includes Next entries but
remains display-only history and never changes decay, freshness, lane, tier,
Today, schedule, or priority.

**Why.** Bryan asked to "start prompting for an optional rotten log entry
when next Obsidian tasks are marked fresh". Next work has not started, so a
keep reason records why the task remains there instead of asking what work
was completed.

Rejected alternatives:

- **A Work Log summary for Next.** "What did you get done?" does not fit
  unstarted work.
- **Counting Next keeps toward `[keeps::]`.** The streak remains the
  consecutive due Ready confirmation count.
- **Blank reason logs `🤷 no reason given`.** A blank Next answer stamps
  without a Rotten Log entry.
- **A separate Next log.** Next keep reasons share the task-local Rotten Log
  and its display-only history count.

Evidence: `plan:202611/next_keep_rotten_log.md`; bob-navigation-hotkeys
2.64.0; bob-ledger-tools 1.81.0.

**Cost.** Up to about 15 extra Enter presses per morning at the Next cap.
Rotten Log entries are no longer only counted keeps.

**Reopens when.** Next reasons go mostly blank for a month, or Bryan wants
Pending keeps in the Rotten Log too.

Links [[decisions/rotten-keeps-are-logged]],
[[decisions/task-lanes-are-sticky]], and [[decisions/review-walk-is-tiered]].

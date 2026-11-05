---
summary: Any numeric close list selects explicit scope; Event Tasks keep clock outcomes.
---

aka numeric close selection, explicit close list

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, and the Bob vault.

**Claim.** Any numeric Task Link list on a Pomodoro close selects an explicit
set: an `x<K>` in-progress list, `x*<K>` park list, `x!<K>` complete list, or
`x~<K>` drop list. The listed links follow their selected action; an
unlisted plain Task Link is deferred. Authored marker outcomes and wildcard
actions keep their meanings. An unlisted Event Task keeps its clock-based
outcome, as specified by [[decisions/event-task-links-close-by-the-clock]].

**Why.** Before this change, only an in-progress list or a nonempty park list
activated explicit mode. A complete or drop list acted on its numbered links
while unlisted plain links still kept their ledger outcomes. One rule makes
every numeric list define scope, so `x!<K>` and `x~<K>` defer unlisted plain
links just as `x<K>` and `x*<K>` do. The Event Task clock rule keeps calendar
outcomes consistent with the meeting and session clocks.

Rejected alternatives:

- **Keep explicit mode limited to in-progress and park lists.** Then `x!<K>`
  and `x~<K>` would act on listed links but leave unlisted plain links
  implicit, unlike the other numeric lists.
- **Defer unlisted Event Tasks in explicit mode.** Calendar outcomes continue
  to follow their meeting and session clocks under
  [[decisions/event-task-links-close-by-the-clock]].

**Cost.** `x!<K>` and `x~<K>` defer unlisted plain Task Links, so continuing
work on another plain link requires listing it in an in-progress selection.
Work Log lines resolve against the listed worked set; a drop-only list works
no task and cannot carry a log by itself.

**Reopens when.** Bryan finds drop lists routinely need to preserve every
unlisted plain link, or wants Event Tasks to follow explicit selection scope
instead of their clock outcome.

Evidence: `plan:202611/close_selection_explicit_mode.md`;
`docs/capture.md` § Pomodoro close selection; `src/native/capture_pomodoro_close/selection_tests.rs`;
`tests/fixtures/event_task_close/vectors.json`.

[[decisions/event-task-links-close-by-the-clock]]

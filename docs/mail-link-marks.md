# Mail link marks

This guide is the authoritative display contract for Gmail reply signals on
task lines. The JavaScript mirror lives in bob-ledger-tools:
`src/142i-mail-link-marks.js` (pure core),
`src/271e-plugin-mail-link-marks.js` (Live Preview, refresh, and toggle),
`src/271f-plugin-mail-link-marks-reading.js` (rendered views), and
`src/142j-no-reply-pills.js` plus `src/264b-plugin-no-reply-pills.js` (the
no-reply pill, through the task-tag-marks machinery). The "Also: \<who\>
replied" tooltip pip is part of the blocked-reason tooltip in
`src/142c-block-marks-model.js`. Tests run the conformance vectors below
verbatim. There is no api namespace.

All signals are display-only. They read the linked mail note's frontmatter
through the metadata cache (`turn`, `turn_since`, `who`) and never write.
Without the plugin the vault looks exactly as it does today.

## What you see

A task whose mail note says the ball is in Bryan's court wears a quiet reply
badge after the mail link. A Ready row whose `#awaiting_reply` hold is due
or overdue replaces that tag with a muted no-reply pill. A row still held
`[?]` for another reason after they reply adds one tooltip line on the
blocked-reason mark.

```text
they replied
  stored:   - [ ] #task [[mail/…|✉️ Lease renewal · Ada]] [fresh:: 2026-11-05] [unblocked:: 2026-11-09]
  rendered: [ ] ✉️ Lease renewal · Ada ↩ Ada · 2h                              TICKLER

nudge due (no reply)
  stored:   - [ ] #task #awaiting_reply [[…|✉️ Lease renewal · Ada]] [scheduled:: 2026-11-10]
  rendered: [ ] ✉️ Lease renewal · Ada   ✈ no reply since Sat, Nov 7            TICKLER

still blocked, they replied
  stored:   - [?] #task [[…|✉️ Lease renewal · Ada]] [dependsOn:: x]
  tooltip:  Blocked · after … · Also: Ada replied
```

## Principles

- **Display-only.** Nothing is written. Frontmatter on the mail note is
  the only extra input, read through the metadata cache, never from disk
  during render.
- **Truthful or neutral.** A badge or pill appears only when the linked
  note is a mail note (`type: [[mail]]` or a string `gmail_thread_id`) and
  the turn facts prove the case. Unresolved links, missing cache, unknown
  turn, and thrown errors show no signal and no warning.
- **One lookup, three surfaces.** The first mail wikilink on the line
  supplies `who`, `turn`, and `turn_since` for the badge, the pill, and
  the tooltip pip.
- **Reversible.** The session toggle `toggle-mail-link-marks` restores
  today's look for the badge. The no-reply pill follows Toggle task tag
  marks. The pip follows Toggle blocked-reason marks.

## Reply badge

The badge sits after the task's mail link: `↩ <who> · <age>`, for example
`↩ Ada · 2h`. `<who>` is the mail note's `who`, first line, trimmed, with
`::` folded to `:`. It is omitted when unknown (`↩ 2h`). `<age>` is the
elapsed time from `turn_since` to now: `now`, `Nm`, `Nh`, or `Nd`.

It shows when **all** of these hold:

1. The line is a `#task` line (quote-aware `freshnessTaskStatus`).
2. A non-embedded wikilink on the line resolves to a mail note whose
   `turn` is `yours` (their move or a bounce, so the ball is in Bryan's
   court).
3. `turn_since` is later than the task's `[fresh::]` date. A same-day tie
   counts when `[unblocked:: today]` is on the line, matching the walk's
   `fresh ≤ unblocked ≤ today` tie rule. A task with no `[fresh::]` shows
   the badge whenever `turn: yours`.
4. The session toggle is on.

The badge disappears after the next freshness stamp (a `[fresh::]` on or
after `turn_since`'s calendar date, with no same-day `[unblocked:: today]`
tie). `turn: theirs`, a missing mail note, and a cold cache show nothing.

Host:
`span.bob-mail-reply-badge[role=img][aria-label][data-tooltip-position=top]`,
text content exactly the badge label. No `title` attribute. Listener-free
DOM builder. Gate: `body.bob-mail-link-marks`.

Live Preview: a zero-width `Decoration.widget` at the closing `]]`,
`side: 1`. The cursor on the link reveals the raw `[[…]]`; the badge
stays put. Reading view inserts the host after the link's
`a.internal-link`.

## No-reply pill

On a Ready (`[ ]`) row with an exact `#awaiting_reply` token and a
canonical `[scheduled::]` of today or earlier, the tag pill is replaced
with a muted `✈ no reply since <Sat, Nov 7>`, using the mail note's
`turn_since` calendar date through `freshnessShortDate`. With no
`turn_since` it reads `✈ no reply yet`. The pill is the overdue signal.

It does not fire on `[?]` rows (the hollow plane already says waiting),
on a future scheduled date, on a near-miss such as `#awaiting_reply_old`,
or on a tag inside inline code. Tasks query results keep the native
`#awaiting_reply` pill: CSS cannot know `turn_since`, and no JavaScript
touches Tasks' DOM.

The pill rides Toggle task tag marks (`body.bob-task-tag-marks`) and
uses the same reveal-on-click as the `#task` glyph: the cursor or a click
exposes the raw `#awaiting_reply` for editing.

## Also-replied tooltip pip

A row still held `[?]` for another reason (`dep`, `prj`, a date) after a
reply — the mail note has `turn: yours`, and `#awaiting_reply` is gone —
appends one tooltip line: `Also: <who> replied`. With no `who` it reads
`Also: replied`. The line never contains `::`. It is omitted when the
awaiting reason is still live (that case already has
`Also awaiting reply from <who> · nudge …`).

## Anatomy and look

- Reply badge: interface font, `0.8em`, muted ink
  (`color-mix` of `--text-muted` toward `--text-normal`), no pill
  background at rest, `margin-inline-start: 0.35em`,
  `user-select: none`, `cursor: default`. It never changes line height.
- No-reply pill: interface font, `0.8em`, muted ink, a quiet capsule
  (`color-mix` of `--text-muted` at 12% on transparent),
  `border-radius: 999px`, `padding: 0.05em 0.45em`.
- Toggles: `body:not(.bob-mail-link-marks) .bob-mail-reply-badge { display: none; }`.
  The no-reply pill is gone when task tag marks are off.

## Conformance vectors

Today is Sat 2026-11-07. `A` is `[[mail/2026/261107_lease_renewal|✉️ Lease renewal · Ada]]`.
The linked note has `type: [[mail]]`, `who: Ada`, unless a row says otherwise.
`now` is `2026-11-07T16:00:00` when an age is asserted.

**ML — reply badge.**

| #   | Line / mail facts                                                                                         | Expected                          |
| --- | --------------------------------------------------------------------------------------------------------- | --------------------------------- |
| ML1 | `- [ ] #task A`, `turn: yours`, `turn_since: 2026-11-07T14:00:00`, no `[fresh::]`                          | `↩ Ada · 2h`                      |
| ML2 | ML1 plus `[fresh:: 2026-11-05]`                                                                           | `↩ Ada · 2h`                      |
| ML3 | ML1 plus `[fresh:: 2026-11-07] [unblocked:: 2026-11-07]`                                                   | `↩ Ada · 2h` (same-day tie)       |
| ML4 | ML1 plus `[fresh:: 2026-11-07]`, no `[unblocked::]`                                                        | none                              |
| ML5 | ML1 plus `[fresh:: 2026-11-07]`, `turn_since: 2026-11-06T18:00:00`                                         | none (stamped after the reply)    |
| ML6 | ML1 with `turn: theirs`                                                                                   | none                              |
| ML7 | `- [ ] #task No mail link`                                                                                | none                              |
| ML8 | ML1, toggle off                                                                                           | none                              |
| ML9 | ML1 with empty `who`                                                                                      | `↩ 2h`                            |
| ML10 | ``- [ ] #task `[[mail/x]]` ``, `turn: yours`                                                              | none (inline code)                |

**NR — no-reply pill.**

| #   | Line / mail facts                                                                      | Expected                            |
| --- | -------------------------------------------------------------------------------------- | ----------------------------------- |
| NR1 | `- [ ] #task #awaiting_reply A [scheduled:: 2026-11-07]`, `turn_since: 2026-11-04T09:00` | `✈ no reply since Wed, Nov 4`      |
| NR2 | NR1 with no `turn_since`                                                               | `✈ no reply yet`                    |
| NR3 | `- [?] #task #awaiting_reply A [scheduled:: 2026-11-10]`                               | none (the plane owns `[?]`)         |
| NR4 | `- [ ] #task A [scheduled:: 2026-11-05]`                                               | none (no tag)                       |
| NR5 | `- [ ] #task #awaiting_reply A [scheduled:: 2026-11-10]`                               | none (still in the future)          |
| NR6 | `- [x] #task #awaiting_reply A [scheduled:: 2026-11-05]`                               | none (closed)                       |
| NR7 | `- [ ] #task #awaiting_reply_old A [scheduled:: 2026-11-05]`                           | none                                |
| NR8 | NR1, Toggle task tag marks off                                                         | native `#awaiting_reply` pill       |

**AP — Also-replied pip.**

| #   | Line / mail facts                                                                 | Expected tooltip contains      |
| --- | --------------------------------------------------------------------------------- | ------------------------------ |
| AP1 | `- [?] #task A [dependsOn:: x]`, `turn: yours`, `who: Ada`                        | `Also: Ada replied`            |
| AP2 | `- [?] #task #awaiting_reply A [scheduled:: 2026-11-10] [dependsOn:: x]`           | no `replied` (awaiting is live)|
| AP3 | AP1 with `turn: theirs`                                                           | no `replied`                   |
| AP4 | AP1 with empty `who`                                                              | `Also: replied`                |

Verification: ML, NR, and AP are covered by
`scripts/test-ledger-tools-mail-link-marks.cjs` and
`scripts/test-ledger-tools-no-reply-pills.cjs` in bob-plugins, plus the
block-marks tooltip tests. The daily-note Gmail sync trigger is covered by
`scripts/test-ledger-tools-gcal-events.cjs`.

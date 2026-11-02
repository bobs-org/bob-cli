---
keyword: sync-sorts-event-task-links
aliases:
  - gcal Event Task link order
summary: Every bob gcal sync run reorders each Pomodoro entry's same-note Event Task links by meeting span; other children keep their slots.
metadata:
  status: accepted
  decided: 2026-11-02
title: Sync Sorts Event Task Links By Meeting Time
---

# Sync Sorts Event Task Links By Meeting Time

**Applies to.** bob-cli and the vault.

**Claim.** Every `bob gcal sync` run orders each Pomodoro entry's dedicated
same-note Event Task links by meeting span. A link qualifies when it is a
direct-child Task Link recognized by `capture_pomodoro_start::session_links`,
including 🍅 and struck links, its path is empty or names this note's
`YYYYMMDD` stem, and its block ID identifies exactly one Event Task in this
note's `## Events` section. Links sort by meeting start, then timeline end,
then Events row order; links to the same block ID remain stable. Each link
moves with its child subtree. Other children, including notes and non-Event
Task links, keep their positions, as do gaps between child blocks. Entry
headers and total line count stay unchanged. A sort-only run writes the note
and refreshes its stamp; a rerun after sorting is a no-op apart from the
normal per-minute stamp rule.

**Why.** Bryan's 2026-11-02 request fixes `2026/20261102.md`, where the 08:00
`^swings` row should sit above the 19:00 `^lincoln-park-w` row. Capture agenda
and lineup order follows Task Link position, so matching Events order also
orders the agenda.

**Rejected alternatives.**

- **Sorted insertion only.** It does not repair a misordered entry or
  hand- or capture-added links.
- **Buffered-span order.** It disagrees with the bold meeting labels and the
  Events section.
- **Sorting every child.** It scrambles notes and non-Event Task lineup order.
- **Grouping Event Task links contiguously.** It moves non-Event children.

**Cost.** A hand reorder of Event Task links is undone on the next sync, and
a sort-only run writes the note.

**Reopens when.** Bryan wants a manual meeting order to stick or link order
should follow buffered spans.

Evidence: `plan:202611/gcal_sort_event_task_links.md`; `docs/gcal.md` § Sync;
`tests/gcal_sync.rs`.

[[decisions:synced-meetings-link-by-phase]]

[[decisions:event-buffers-widen-the-span]]

[[glossary:event-task]]

[[glossary:task-link]]

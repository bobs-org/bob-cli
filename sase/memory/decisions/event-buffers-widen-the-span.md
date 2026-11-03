---
keyword: Event Buffers Widen The Occupied Span
aliases:
  - event-buffers-widen-the-span
  - event buffers
  - buffer field
summary:
  "The buffered span is what the event occupies; the meeting span is when
  the meeting happens."
metadata:
  status: superseded-in-part
  decided: 2026-11-02
  superseded_by: decisions:buffer-reasons-ride-description-bullets
title: Event Buffers Widen The Occupied Span
---

# Event Buffers Widen The Occupied Span

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, and the vault.

**Claim.** Every Event Task has its **meeting span**, the bold
`**1200-1300**`, which is never rewritten. A `[buffer:: -30m +15m]` field
adds a **buffered span**: `max(00:00, start − before)` to `end + after`.
The buffered span is what the event occupies. The meeting span is when the
meeting happens.

| Uses the buffered span | Keeps the meeting span |
| --- | --- |
| MEET event run (`=` default range, early start, far-run warning, run clustering) | Timed close classification (`ended_inside`/`running`/…, shared `event_task_close` vectors) |
| Run-into-event warnings and fit drafts (capture, Obsidian Notice) | Agenda row `phase` (`ended`/`running`/`upcoming`) and `focus` |
| `bob gcal sync` phases (past/current/future) and the MEET clusters and spans it writes | The bold span text, its journal hash, and Google ownership of the time |
| Agenda `gap`, `conflict`, `start_hint`, `refresh_at` | Row titles, `#untracked`, roots |

Buffers are read from Google description **directive lines** (combinable,
HTML-safe `@route` / `#untracked` / `-N` / `+M` tokens) and written as one
Google-owned `[buffer::]` field. A side that is zero is omitted; an event
without buffers has no field.

**Why.** Bryan asked to reserve travel and prep around a meeting from the
Google description (`-30` / `+15`) without losing the meeting's real time.
Rewriting the bold span would break span parsers in Rust, JS, and Swift.
Glyph tokens such as `⇤30m` are forgeable from titles and need new escapes
and strippers in two languages. Two fields `[before::]` `[after::]` split
one fact and are noisier. `escape_title` already neutralizes `::`, so a
title cannot forge the field; Rust and JS already scan inline fields and
strip them from titles.

Rejected alternatives:

- **Rewriting the bold span to the buffered span.** It loses the meeting
  time and breaks span parsers in Rust, JS, and Swift.
- **`⇤30m`-style glyph tokens.** They are forgeable from titles and need
  new escapes and strippers in two languages.
- **Two fields `[before::]` `[after::]`.** They are noisier and split one
  fact in two.

**Cost.** A pre-buffer stops at midnight. The first value for each side
wins. Numbers in titles are never buffers. Timed close keeps the bare
meeting clock, so a MEET closed at 13:00 still completes a 12:00–13:00
lunch whose `+15` drive runs to 13:15. Description-only grammar means a
description you cannot edit cannot take a Google-owned buffer.

**Reopens when.** Buffers need to wrap past midnight, live in the title, or
widen timed close.

Evidence: `plan:202611/gcal_event_buffers.md`; `docs/gcal.md` § Untracked
events, § Root selection; `docs/capture.md`;
`tests/fixtures/event_buffer/vectors.json`.

[[decisions:synced-meetings-link-by-phase]]

[[decisions:calendar-events-are-daily-note-event-tasks]]

[[decisions:event-task-links-close-by-the-clock]]

[[decisions:untracked-sessions-omit-t-field]]

[[glossary:event-task]]

Superseded in part: buffers are also read from reason lines (`-30 Drive to Kelly's`), which write a labelled reason bullet. See [[decisions:buffer-reasons-ride-description-bullets]].

---
title: "Plan an event you don't own"
description: "RSVP, then attach Bob-only route, clock, buffer, and prose to a Google Calendar event you cannot edit."
tags:
  - CLI
---

# Plan an event you don't own

This guide attaches Bob-only metadata to a Google Calendar event whose
title and description you cannot edit: a guest invite, a locked family
event, or an invitation you have not answered yet. You write the same
`@route`, `#off`/`#on`, `-30 Drive there`, and prose lines you would put
in Google. They live in Bob, beat Google's description field by field, and
land on that event's Event Task when sync writes the row. Bob never writes
the note to Google.

Exact flags, grammar, and JSON live in
[Private event notes](../gcal-notes.md). Connect Calendar first with
[Connect Google Calendar](connect-google-calendar.md).

## Before you start

Install the CLI, point it at your vault, and confirm `bob gcal doctor`
passes. RSVP in Google Calendar (Yes, Maybe, or leave it awaiting a
reply). A note never RSVPs and never creates a row; it only describes an
event that can become an Event Task.

## 1. RSVP, then write the note

```bash
bob gcal note deerwood @family -30 Drive to Deerwood +15 Drive home
```

Bob picks the matching event in the next 90 days, prints the canonical
lines, a time rail for the occupied span, and the Event Task row sync will
write, then saves the note. Quote the text when it contains spaces. Use
`--` before the text when a query word looks like a directive.

A recurring match asks **This event** or **All events** on a TTY. Off a
TTY, pass `-s event` or `-s series`. `-s series` on a one-off event is a
usage error.

Preview without saving with `-d`. Clear with `-c`.

## 2. Check what you noted

```bash
bob gcal note
bob gcal note deerwood
bob gcal list
```

`bob gcal note` lists private notes for the next 90 days. Showing one
event prints the canonical lines, the rail, and the landing row. `bob gcal
list` marks a noted event with a dim `✎` and shows the root as
`✎ @family`.

## 3. Adopt Bob lines that live in someone else's description

If an external invite already carries `@route`, `#off`/`#on`, or `-30`
reason lines in the organizer's Google description, copy those directive
and reason lines into Bob:

```bash
bob gcal note -A -i EVENT_ID
```

Prose stays in Google. The adopted lines then survive the organizer's next
description edit. If that event already has a private note, edit it
instead of adopting over it.

## 4. Route from your phone with event colour

A private note is a desktop or SSH command. Until you are at a keyboard,
recolour the event in Google Calendar on your phone and map that colour in
config:

```yaml
gcal:
  colors:
    grape: family
    tomato: body
```

Colour is the root source after a note or `@route` token and before the
calendar's root. Replace it with a private note once you can run
`bob gcal note`.

## On your Mac

Bob Mac Capture ships an [Event notes](../reference/mac-capture/event-notes.md)
surface for the same contract. Press Command-E from any capture draft — empty or
not — pick the event, choose This event or All events for a recurring match, and
edit with Bob's highlights and time rail. Return saves, Command-Delete clears the
saved scope, and Command-Z undoes the last save. Bob still performs the writer-host
sync; the Mac never issues a second one.

## What "lands" means

Saving the note stores it now. Sync writes it onto the Event Task when
that day's daily note exists and the event is a task (Yes, or Maybe with
`gcal.tentative: task`).

| You save the note… | Result |
| --- | --- |
| Before that day's note exists | It lands when that day syncs |
| On an event awaiting your reply | It waits until you answer Yes |
| On an event answered Maybe | It waits until Yes, unless `gcal.tentative: task` |
| On today's task on the writer host | The command syncs at once (`↻ applied to …`) |
| On another host | It is saved; the writer applies it on the next sync |

A series note applies to every occurrence and reports the next one's
status. Your vault edit of the row still wins; sync reports that once.

## If it goes wrong

| Symptom | What to do |
| --- | --- |
| No match | Widen the query, pass `-D DAY` for a past day, or paste the Google Calendar URL. |
| Several matches off a TTY | Pass `-i ID` and `-C` if the id exists on two calendars. |
| Recurring event, no TTY | Pass `-s event` or `-s series`. |
| Unknown `@route` | The error lists close routes. Use an area or root project that resolves from that day's note. |
| Note saved, row unchanged | The daily note may not exist yet, you have not answered Yes, or you are not on the writer host. Re-run `bob gcal sync` there when the day exists. |
| `held … private note could not be read` | Fix malformed `.bob/gcal/*.jsonl` lines, then `bob gcal doctor` and `bob gcal sync`. |

## Related

- [Private event notes](../gcal-notes.md)
- [Bob Gcal](../gcal.md)
- [Connect Google Calendar](connect-google-calendar.md)
- [bob gcal](../reference/cli/gcal.md)

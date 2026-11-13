# External event acknowledgements

Bob asks once about every event you can't describe in Google: press `a` when
Bob's handling is right, or `n` to write a private note. Until you answer,
that Event Task carries an `#unacked` tag.

The acknowledgement is a soft review reminder. Sync still creates an
eligible Event Task and its Pomodoro link; it never holds an event back or
writes to Google.

## Contents

- [Model](#model)
- [Command](#command)
- [Keys](#keys)
- [Storage](#storage)
- [Precedence](#precedence)
- [Reliability](#reliability)
- [JSON](#json)
- [Exit codes](#exit-codes)
- [Known limits](#known-limits)

## Model

An event is external when its organizer is not you or Google classifies it as
`fromGmail` (for example, a flight or reservation). A missing organizer is
treated as external. Review applies to events that Bob classifies as Event
Tasks, not ordinary context rows. `gcal.ack: external` is the default;
`gcal.ack: off` disables the review queue and warning tags.

The queue shows eligible, unanswered external Event Tasks that have not
already been removed from their daily note. It sorts soonest first and, by
default, covers today through the next six local days. `-a` also includes
eligible unanswered invitations and Maybe context events for early review.
Events that have already ended are excluded. It fetches from Google but never
creates future daily notes.

## Command

```text
bob gcal ack [-a|--all] [-b|--bob-dir DIR] [-C|--calendar ID] [-c|--clear]
             [-D|--day DAY] [-d|--dry-run] [-f|--format human|json] [-h|--help]
             [-i|--event-id ID] [-l|--list] [-n|--next-n-days N]
             [-q|--quiet] [-s|--scope event|series] [QUERY]
```

| Form | Behavior |
| --- | --- |
| `bob gcal ack` on a TTY | Starts the one-key review session. |
| Bare `ack` off a TTY | Lists the queue with a hint; it does not write. |
| `-n N` | Looks forward N local days, including the anchor (`1..=366`, default `7`). Unlike sync's `-n`, this looks forward. |
| `-D DAY` | Sets the local anchor day (`today`, `tomorrow`, `yesterday`, or a date). |
| `-a` | Includes eligible unanswered invitations and Maybe context events. |
| `-l` | Lists without prompting or writing. |
| `QUERY` or `-i ID [-C CAL]` | Selects one event using the note command's title matching and `eid=` URL decoding. Explicit IDs can be outside the queue window. |
| `-c QUERY` or `-c -i ID` | Clears the selected explicit receipt. It cannot erase note history or a receipt at another scope. |
| `-d` or `-f json` | Previews or lists without prompting or writing. JSON is always read-only. |

Recurring selections ask for a scope on a TTY. Without a TTY, pass
`-s event|series`. The review key `a` defaults to all events in a series;
notes default to this event. The `-c` clear needs an event selector and
clears only the selected event or series receipt.

An explicit `-n` or `-a` walk needs a TTY. Use `-l`, `-f json`, or a
one-shot selector such as `-i ID` in scripts.

## Keys

| Key | Action |
| --- | --- |
| `a` | Acknowledge the event. For a recurring event, choose this event or all events; the default is all events. |
| `n` | Write a one-line private note using the same text grammar as `bob gcal note`. An empty line returns to the card. |
| `e` | Edit the note in `$VISUAL`, then `$EDITOR`, or `vi` if neither is set. |
| `s` | Skip this run; the event returns next time. |
| `o` | Open the event in Google Calendar. |
| `A` | Adopt organizer-description directives when the card shows the fragile-directives hint. |
| `q`, Esc | Quit. Answers already given are saved. |
| Ctrl-C | Quit with exit code `130`; answers already given are saved. |
| `?` | Show the available keys. |

Each answer is written as soon as it is given. Repeated keys are drained at
card boundaries so a held key does not answer the following event. The
terminal is restored on quit, error, interrupt, or panic.

## Storage

Explicit receipts are append-only records in the existing per-host journal,
`.bob/gcal/<host>.jsonl`:

```json
{
  "v": 1,
  "ts": "2026-11-16T07:12:04-05:00",
  "host": "athena",
  "kind": "gcal.ack",
  "account": "you@example.com",
  "key": { "calendar_id": "primary", "event_id": "event-id" },
  "scope": "event",
  "acked": true,
  "via": "ack",
  "seen": { "title": "Screening", "start": "2026-11-17T12:00:00-05:00", "end": "2026-11-17T12:15:00-05:00", "original_start": null }
}
```

An event receipt keys on `{calendar_id, event_id}`. A series receipt keys on
`{calendar_id, series}` and covers its occurrences. The stored `seen` values
are display and stale-detection data, never a way to transfer an answer to a
different event. `acked: false` is a deliberate clear. Records from a
different resolved Google account do not apply.

Sync writes Bob's exact `#unacked` token as the final tag before an Event
Task's block ID, on open rows that still need review. The token belongs to
Bob while present: removing it by hand is an acknowledgement, recorded on
the next sync with `via: row`, and Bob will not re-add it. Sync removes its
tag when the event is answered or the row closes. Context rows and events
born done do not carry the tag.

Any host with the Calendar client can append receipts, like private notes.
Only the `gcal.writer` host applies the tag change on its next sync. Acks
survive `bob gcal sync -F` and `gcal.reset`.

## Precedence

Bob decides review status in this order:

| Evidence | Status |
| --- | --- |
| A live event- or series-scope receipt | `acked` |
| Any persisted nonempty private note history for the event or its series | `noted` |
| No live receipt and no such note history | `unacked` |
| Review is disabled or not required for this row | `not_required` |

Every existing note record counts. Once a note for an event has held
nonempty text, clearing it does not un-ack that event. A nonempty series note
answers existing and future instances in that series, and clearing the note
does not remove that history. Saving a note does not create an extra ack
record. See [Private event notes](gcal-notes.md#storage).

Clearing an explicit receipt removes only the selected receipt. If an event
or series note has ever held nonempty text, review remains answered as
`noted`; another live receipt at a wider scope can also keep it answered.

## Reliability

The receipt saves the event's title and meeting times at review time. An
event-scope receipt is reported stale when its current start or end instant
differs; display-only timezone formatting and title changes do not make it
stale. Series receipts are not marked stale. A stale receipt still counts as
reviewed: v1 displays `ack stale · moved since ack?` in `list`, `ack`, and
`doctor`, and sets `ack_stale: true` in JSON, but does not ask again
automatically. Reviewing it again refreshes the saved times. The reminder is
“Ack records that you reviewed this event; it does not approve later
changes.”

`bob gcal doctor` reports receipt counts, malformed or foreign-account
records, orphaned receipts, stale event receipts, and lookup-limit skips.
It does not silently prune receipts when an event disappears.

## JSON

`bob gcal ack -l -f json` returns `schema_version`, `ok`, `action: "list"`,
`preview`, `window`, `queue`, `waiting`, `stale`, `counts`, `complete`, and
`warnings`. Queue rows carry event identity, title, time, organizer, whether
the event is external, where it lands, root and source, buffer, clock, an
`ack` summary, and `ack_stale`. Selected-event previews add
`proposed_action`; they never write. Note text is never returned.

The `ack` summary is `{state, scope, via, at}`. `state` is `unacked`,
`acked`, `noted`, or `not_required`; `scope`, `via`, and `at` are `null`
when there is no applicable receipt or current note metadata. The `waiting`
lane is a read-only count and list of unanswered invitations. The `stale`
list identifies moved event-scope receipts.

`bob gcal list -f json` and `bob gcal sync -f json` add the same `ack` and
`ack_stale` fields to their event or row objects, and expose `counts.unacked`.
No ack or note text appears in these JSON surfaces.

## Exit codes

- `0` — list, empty queue, completed action, or voluntary quit. Queue JSON
  uses `complete: false` when skipped work remains.
- `1` — Google, network, or journal failure.
- `2` — usage or setup error, ambiguous selection, a non-TTY walk, or a
  recurring one-shot selection without `-s` off a TTY.
- `130` — Ctrl-C after saving completed answers and restoring the terminal.

## Known limits

- Review is limited to events that can land as Event Tasks. `-a` adds
  unanswered invitations and eligible Maybe context events for early review.
- Acknowledgement does not change RSVP state or write to Google.
- A changed event time produces a stale notice, not an automatic re-review.
- Older bob versions do not recognize `gcal.ack` journal records; upgrade
  the writer before using ack records across hosts. See
  [Bob Gcal known limits](gcal.md#known-limits).

# Private event notes

A private event note is a description of a Google Calendar event that only Bob
reads. Write the same `@route`, `#off`/`#on`, `-30 Drive there`, and prose
lines you would put in Google, and they beat Google's description on that
event's Event Task, field by field. Google keeps the time, the title, and the
📅/📹/📍 links. Bob never writes the note to Google.

The note is keyed by the event's Google identity, so you can write it weeks
before the daily note (and the Event Task) exists. Sync applies it when it
writes the row. Any event qualifies: events you own, locked guest events, and
invitations still awaiting a reply or answered Maybe. A note never changes
inclusion, never RSVPs, and never creates a row.

List, show, preview, write, clear, editor, and Google-description adoption are
available in `bob gcal note`. Mac Capture Event note mode is the remaining
client work.

## Contents

- [Model](#model)
- [Grammar](#grammar)
- [Precedence](#precedence)
- [Storage](#storage)
- [Ownership](#ownership)
- [Reliability](#reliability)
- [Command](#command)
- [When a note lands](#when-a-note-lands)
- [JSON](#json)

## Model

The object is the private event note, not the Event Task. The row is only the
note's projection: byte-for-byte the row the same lines would produce in a
Google description. There is no provenance glyph in the vault.

Two scopes ship:

- **This event** (`scope: event`) keys on `{calendar_id, event_id}`. A
  recurring instance's id already embeds its original start.
- **All events** (`scope: series`) keys on `{calendar_id, series}`, where
  `series` is Google's `recurringEventId`.

Use the configured calendar id, as `gcal.row` does. Never key by title, date,
path, or block id.

`gcal.description: off` hides note bullets too: one kill switch. Note
directives still apply, just as Google's do. `sync -F` keeps notes and
reapplies them. Notes are never pruned silently.

## Grammar

A note's canonical text is description grammar, parsed by the **plain-text**
tokenizer (never the HTML sniffing path):

```text
@body
-30 I need to walk home from the park
Bring the stroller
```

- **Directive line** (`@route`, `#off`, `#on`, `-N`, `+M`, `-0`/`+0`): sets
  the root, the clock tag, and the buffers. `-0`/`+0` explicitly clear a side
  that Google's description set.
- **Reason line** (`-30 …`, `+15 …`): sets one side and writes its `↤`/`↦`
  bullet.
- **Any other line** is prose and becomes a bullet, with the same inert
  escaping Google bullets get.

Canonical text holds the directive line first. It uses canonical spellings:
`#off`, `#on`, ASCII `-`, no `m` suffix, and the resolved route name. The
reason lines follow, before then after, then the prose lines in input order.
Directive lines never render, so the reordering is invisible in the vault.

## Precedence

Each field takes the first layer that sets it:

| Field | 1 | 2 | 3 | then (unchanged) |
| --- | --- | --- | --- | --- |
| Root | ✎ this event | ✎ all events | learned series memory | title `@route`, description `@route`, colour, calendar, default |
| Clock tag (`#off`/`#on`) | ✎ this event | ✎ all events | Google title or description tag | root walk, then on-clock |
| Before buffer | ✎ this event | ✎ all events | Google description | none |
| After buffer | ✎ this event | ✎ all events | Google description | none |
| Reason, per side | travels with the winning value | | | |
| Prose bullets | ✎ this event lines, then ✎ all events lines, then Google's copied lines | | | |

A layer _sets_ a field as follows:

- root: it has a route
- clock: it has `#off` or `#on`
- a side: it has a directive or reason value for that side, `0` included

If a note sets a side without a reason, Google's reason for that side is
dropped, not inherited. Within one layer, today's description rules hold:
`#on` beats off, and the first value per side wins.

Note roots use source kind `note` / `series_note`, ranked first. They behave
like a title `@route` for rerouting: an untouched `[root::]` follows a
note-derived root, while your `[root::]` edit wins and is reported once
(`kept_root`). A note route that no longer validates is a warning and falls
through to the next candidate. When a note route wins and the title ends in a
resolvable `@route` token, that token is still stripped from the displayed
title. Source labels are `✎ @family` (this event) and `✎ all events @abby`
(series).

The description block order is:

1. `↤` reason, then `↦` reason
2. note prose (event scope, then series)
3. Google's copied lines, capped by `gcal.description_lines`
4. the overflow line

Note reasons and prose never count toward the cap. For an oversized Google
description, the block is the reasons, the note prose, then the existing
fallback bullet.

## Storage

Notes live in the existing per-host journal, `.bob/gcal/<host>.jsonl`
(union-merged and already tracked). Any host may append a note. Only the
`gcal.writer` host applies notes, through sync.

```json
{
  "v": 1,
  "ts": "2026-11-09T18:02:11-05:00",
  "host": "athena",
  "kind": "gcal.note",
  "account": "you@example.com",
  "key": { "calendar_id": "primary", "event_id": "st49ogf0k0jiciob1ebgj1aupo" },
  "scope": "event",
  "text": "@family\n-30 Drive to Deerwood\n+15 Drive home",
  "seen": {
    "title": "Deerwood. Christmas breakfast",
    "start": "2026-12-13T10:00:00-05:00",
    "ical_uid": "…",
    "original_start": null
  }
}
```

- **Fold.** Last writer wins, ordered by `(ts, host, path, line)` exactly like
  series roots. `text: ""` is a clear. The fold remembers that a key was
  cleared, as opposed to never noted. Records are append-only.
- **Malformed.** Text larger than 2048 bytes, an unknown scope, or a missing
  key part makes the record malformed.
- **Account guard.** A note whose `account` differs from the snapshot's
  resolved account is never applied.
- **`seen`** is for display and repair only. Never use it to move a note to
  another event.
- **Compatibility.** `load_state` skips `gcal.note`: it does not fold it as an
  occurrence or move `newest`. `malformed_count` accepts a valid note.
  `sync -F` resets day state only; notes survive.

Note text never appears in `bob gcal list` or `bob gcal sync` JSON, which
carry only summaries (`scope`, `fields`, `bullet_lines`). Only `bob gcal note`
itself and `sync -d` diffs show the words.

## Ownership

No new ownership rule. The note is layered over the description before
rendering. The row then follows the unchanged rule: it follows while
untouched, your edit wins, and the edit is reported once. A note written after
the row exists therefore behaves like a Google-side description change.

See [Who owns each part of a row](gcal.md#who-owns-each-part-of-a-row) in
[Bob Gcal](gcal.md): the Owner column for `[root::]`, `[buffer::]`,
`#off`/`#on`, and the description bullets is "your private note, then Google,
until you edit it".

## Reliability

Every `gcal.row` a private note shaped carries `note: {hash, fields}`: the
short hash of the applied canonical note texts and the subset of
`root`, `clock`, `before`, `after`, and `bullets` the note contributed. The
field is absent otherwise. When the journal reports malformed gcal lines and
the current fold holds neither a live note nor a deliberate clear for that
key, sync holds the row: it skips span, root, and description updates, carries
the journal record forward unchanged, and warns on every run while the
condition lasts (`held …: its private note could not be read (N malformed
gcal journal lines) · bob gcal doctor`). A deliberate clear is never held.
When `kept_description` fires and the journaled note hash differs from the
fresh one, sync reports `kept your edited description bullets; your private
note changed` instead of the Google wording.

## Command

```text
bob gcal note [-A|--adopt] [-a|--all] [-b|--bob-dir DIR] [-C|--calendar ID] [-c|--clear]
              [-D|--day DAY] [-d|--dry-run] [-e|--edit] [-f|--format human|json] [-h|--help] [-i|--event-id ID] [-l|--list] [-s|--scope event|series]
              [QUERY...] [[--] TEXT...]
```

The option pre-pass runs before clap, so `-30`, `-30m`, and `+15` are never
read as flags, and `bob gcal note deerwood -c` still works. Tokens that are
this command's options are honored anywhere before the first `--`. A token
matching `^[-−–+]\d` is never an option. After `--`, every token is TEXT
verbatim. The positional words before TEXT form QUERY; TEXT starts at the
first directive token (`@word`, `#off`, `#on`, a deprecated off alias, or
`±N[m]`) or after `--`. An unknown option-like token is a clap usage error
(exit 2).

Modes:

- Bare `bob gcal note`, or `-l`: list notes.
- `-l -a`: list every upcoming event, noted or not. This is the Mac picker
  source.
- `-l QUERY`: filter the listing.
- QUERY without TEXT: show the event, effective note fields, time rail, and
  the row preview.
- QUERY with TEXT, or `-i … -- TEXT`: normalize, validate, preview, and save
  the private note. TEXT words are joined with spaces; use a quoted argument
  or `--` for multiline input.
- `-d`: validate and preview without saving. With no TEXT, it previews the
  selected scope's current note.
- `-c`: append a deliberate clear for the selected scope. Recurring events
  offer only scopes that currently have a note.
- `-A`: adopt the Google description's directive and buffer-reason lines as a
  Bob-only note. The adopted text goes through normal route validation and the
  usual preview/save path; prose and Google-owned bullets are not copied.
- `-e`: open the selected scope's current note in `$VISUAL` or `$EDITOR`.
  The scissors marker and help block are removed before validation and save.
  Combining `-A -e` opens an editor prefilled with the adopted Google lines.

Exit status is 0 on success, 1 on gog or network failure, and 2 on usage or
setup errors, no match, an ambiguous match off a TTY, a missing scope off a
TTY, or an invalid note. Saving identical canonical text reports `unchanged`
and appends no journal record. Saving or clearing on the writer host
immediately syncs an existing rendered day; a sync failure leaves the note
saved and prints a warning with `bob gcal sync` as the recovery command.

Selecting the event fetches from today's local midnight to +90 days, per
configured calendar, with all pages. `-D DAY` narrows the window to one day,
past days included. Eligible events are timed, busy, under-24-hour events of
type `default` or `fromGmail`, answered Yes, Maybe, or awaiting a reply.
Matching requires every QUERY word in the displayed title; an exact title
ranks first, then a prefix, then a contains, then soonest first. Recurring
instances of one series group into one candidate. A pasted Google Calendar
URL with `eid=` is base64url-decoded to the event id and looked up across
the configured calendars (`-C` pins the calendar); when no calendar holds
the id, the command falls back to a title search with a warning. `-i ID`
uses the same lookup. Zero matches exit 2 with a hint; several matches open
a numbered picker on a TTY and exit 2 off a TTY with candidates. A recurring
match asks This event / All events on a TTY and needs `-s` off a TTY; `-s
series` on a one-off event exits 2.

List shows `✎ N private notes · next 90 days`, one row per noted event with
its scope (`this event` / `all events`), root hue, buffer, and status
(`waiting for Yes`, `waiting for your reply`, `lands Sun Dec 13`, `on
today's note`). Notes outside the window are looked up by id (capped at 25
lookups); past events are hidden, while cancelled, declined, or not-found
events show `⚠ no longer on your calendar` with the one-line clear command.
Show prints the current canonical note lines plus the effective root,
buffer, and clock. It also shows the time rail and the exact Event Task row
and bullets that sync will write.

## When a note lands

| You save the note… | Result |
| --- | --- |
| Before that day's note exists | It is saved now and lands when the day's note syncs |
| On an event awaiting your reply | It waits until you answer Yes |
| On an event answered Maybe | It waits until Yes, unless `gcal.tentative: task` makes it a task |
| On today's task and the writer host | The command immediately runs sync and reports `↻ applied to …` |
| On another host or without a rendered daily note | It reports that the note is saved and when or where sync can apply it |
| After the row has been edited | Your edit wins; sync reports the kept edit |

For a series note, the message names the series and reports the selected
occurrence's current status.

## JSON

Every event object in list, show, candidates, and write outputs:

```json
{
  "calendar_id": "primary",
  "calendar_name": "Bryan",
  "event_id": "st49…",
  "series_id": null,
  "recurring": false,
  "title": "Deerwood. Christmas breakfast",
  "day": "2026-12-13",
  "start": "2026-12-13T10:00:00-05:00",
  "end": "2026-12-13T12:00:00-05:00",
  "time": "10:00-12:00",
  "answer": "yes",
  "organizer": { "name": "Kelly", "self": false },
  "url": "https://www.google.com/calendar/event?eid=…",
  "lands": "task",
  "note": null
}
```

`lands` is `task`, `context`, `waiting_reply`, or `waiting_maybe`. `note`
holds a summary in listings
(`scope`, `root` with `value` and `hue`, `buffer`, `clock`,
`bullet_lines`) and never holds text. Show mode adds `note.text` and
`note.lines` and carries `action: "show"`. Both show and write responses
include `effective`: root, explicit clock, buffer minutes and winning reason
display text, contributed fields, and meeting and occupied bounds. A reason is
`null` when none wins, including an explicit zero buffer. Writes add their
canonical `note.text`, typed `note.lines`, preview path, row, bullets, and
status. Their `spans` are half-open UTF-8 byte offsets into `input.text` for
route, clock, buffer, reason, prose, separator, and invalid spans. Invalid
JSON returns `ok: false`, `error`, and ranged `errors` without saving.
Listings never carry note text. The list document is
`{schema_version, ok, window: {from, to, start, end}, notes|events, orphans, warnings}`:
`notes` when every row is noted, otherwise `events`. Orphans carry
`calendar_id`, `event_id`, `series_id`, `scope`, `title`, and `reason`.

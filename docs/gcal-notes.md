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

Command write, preview, and Mac Capture Event note mode land in later
phases. This page is the model the engine already applies.

## Contents

- [Model](#model)
- [Grammar](#grammar)
- [Precedence](#precedence)
- [Storage](#storage)
- [Ownership](#ownership)

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

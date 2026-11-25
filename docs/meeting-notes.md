---
title: "Meeting notes (`bob ref meet`)"
description: "List Event Tasks and file meeting notes as a meetings reference with recap ownership, overwrite, and queue semantics."
---

# Meeting notes (`bob ref meet`)

`bob ref meet` lists Event Tasks that need notes and files supplied notes as
a `meetings` reference: a reference note under `ref/meetings/`, a reading
task, an inert `notes from` relation to the event, a full owned recap, and a
meeting-profile PDF.

All examples use fictional meetings (Cedar Labs, Driftwood) in a synthetic
vault. Never paste a real meeting's notes into a bug report.

## Modes

With `-c`, `-i`, or `-e` it files; with none it lists. `-l` explicitly lists
and conflicts with filing-only options. Clipboard (`-c`) and input (`-i`)
conflict.

```bash
bob ref meet
bob ref meet -l -f json
bob ref meet -c 20261124#^cedar-review
bob ref meet -i notes.md 20261124#^cedar-review
bob ref meet -e 20261124#^cedar-review -P job
bob ref meet -c -d 20261124#^cedar-review
```

List is read-only and never touches the clipboard or stdin. Filing reads one
source once: clipboard, `--input FILE` (`-` reads stdin), or the editor.

## Meeting and destination selection

Accept bare/path `note#^id`, aliased wikilinks, `note:id`, leading `~`, a
Google Calendar URL carrying `eid`, or title-query words. Join query
arguments as the existing parser expects. Explicit identities win over query
and automatic matching.

Automatic candidates are started and not cancelled, using the landed 75% /
two-word strong-match and runner-up-below-50% rules. Evidence and
recurring-title ambiguity are preserved. An explicit future or cancelled
event is permitted with a warning. Missing block IDs refuse with an
actionable hint.

List output is newest first with date, span, title, root, missing/filed
state, and the EVENT token. The header counts meetings that need notes.
`--day` uses the gcal note parser (`today`, `tomorrow`, `yesterday`,
`YYYY-MM-DD`); `--window DAYS` defaults to the configured 7 days (1–365).
List queries resolve explicit identities (`note#^id`, wikilink, `note:id`,
leading `~`, Google `eid` URL) through the event parser; a miss yields an
empty list. Title-query words filter titles only.

Filing destination is `--parent NOTE`, else the event root through parent
resolution. An inbox, missing, unresolvable, or ineligible root needs a
choice: off-TTY and JSON clients receive a structured `needs_parent` error
with root and parent candidates. Only a missing or inbox root may be
re-rooted, and only when a valid explicit or prompted parent is chosen. An
override of an ordinary root changes only the reference destination. An
unresolvable but non-inbox root never authorizes rewriting the event.

The stem is the meeting `YYMMDD` plus up to six event-title content words,
with `_2`, `_3`, … collision allocation; `meeting` is the suffix when the
event title has no content words. The derived title is
`Cedar Labs review — Meeting Notes (Tue 2026-11-24, 14:00–14:30)`.
`--title` overrides the derived title without redefining meeting identity.

One reference exists per occurrence: Google event ID when known, otherwise
the resolved event note and block identity. URLs with different sharing
parameters never create duplicates; different recurring occurrences stay
distinct. Same-stem collisions are not dedupe.

## Notes, config, and editing

At most 1 MiB is read with the existing notes reader; empty, binary,
unreadable, invalid UTF-8, and oversized notes refuse with
`notes_unreadable` or `no_notes`. `--expect-sha256 HEX` is a hidden client
guard: a mismatch refuses with `notes_changed` before editor launch,
rendering, or mutation.

Config reuses `meetings:` from
[configuration](reference/configuration.md#meetings): `producer` (unset),
`relation_label` (`notes from`), `status` (`next`), `window_days` (7,
1–365). Status accepts `next`, `ready`, `wip`, `read`. Effect-bearing
relation labels refuse before writes or rendering.

Normalization is structural and deterministic. The full recap excludes
transcripts and escapes notes-derived tokens. Without `--transcript`, no
transcript bytes enter the PDF input or the saved source; with it, the
appendix is included but the note body never is.

The filed note keeps an owned `## Notes` recap between the embed and
`## Highlights`. It starts with a meta line such as `_Tue 2026-11-24 ·
14:00–14:30 · [[20261124#^cedar-review|Cedar Labs review]] · VoiceQuill_`,
then an optional `**Meeting title**`, a `> [!summary] TL;DR` callout,
`### Next steps`, and `###`/`####` sections. An empty filing keeps only the
heading and the meta line.

Editor review (`-e`) opens a temporary normalized draft with the scissors
convention; trusted event and parent identity stays outside the editable
notes. The accepted body parses back into notes and sets the dossier
`edited` flag when the user changed the wording, so the PDF provenance does
not claim unchanged text. Empty output is `no_notes`; editor failure or
cancellation never files. For `-d`, no editor launches: supplied notes
preview; an empty editor-only dry run returns `no_notes` with a hint to
supply notes or drop `-d`.

## Statuses

The new reading task uses `--status next|ready|wip|read`, defaulting to the
configured status. On `--overwrite`, only an explicit `--status` changes the
existing task; otherwise the task keeps its status, residence, and block ID.

## Overwrite

Without `--overwrite`, filing an already-filed occurrence refuses with
`already_filed` (exit 2) naming `{note, task}`. With `--overwrite`, the
existing occurrence is replaced in place: note path, recorded ID, stem,
reading-task residence and block ID, and LINKS bytes are preserved.

Recap ownership is computed from the live note at the transaction's checked
preimage. An edited recap stays byte-for-byte with a warning; an owned
recap is reseeded. Annotated overwrite keeps the recap live; it never
migrates into `ref_history/`. Missing or malformed markers never authorize
deleting user text. All recap changes stay inside the transaction and its
recovery state.

## Dry run, queue, and re-root

`--dry-run` resolves and previews with no vault or store writes, locks, PDF
rendering, network, or model calls.

Filing normally commits one scoped ingest commit
(`bob ref meet: ingest meetings/<stem>`) with the note, PDF, task parent,
and the daily note when a re-root applies. When intake queues (lock, sync,
config, or guard condition), the result is `queued`: it names the PDF, says
`queued` (never `filed`), points at `bob ref scan`, and reports the skipped
re-root. A later `bob ref scan -i` births the stored marker and source
normally and never replays the abandoned re-root intent.

A successful re-root edits the daily note in place without touching the
Google-owned buffer or badge span. An ordinary-root override files without
re-rooting. A changed event causes a typed refusal or queue without a stale
line edit. On recurring series, Bob learns the root for later filings; a
forced reset follows `bob gcal sync -F`.

## Undo

Revert the single ingest commit (`git revert`) or the single overwrite
commit. Queued filings wrote nothing: delete the queued PDF from `xlib/` or
drain it with `bob ref scan -i`. Re-root edits revert with the same commit.

## Human output

List:

```text { .no-copy title="Output" }
Recent meetings · 1 need notes
2026-11-24 14:00–14:30 · Cedar Labs review
  EVENT 20261124#^cedar-review
  root job · missing
```

Preview and filing show meeting, match evidence, destination, title, outline,
and warnings. Success shows the note, reading task and status, relation, PDF
and page count, commit, and the reconcile timing for the event mirror.
Queued output says queued and names `bob ref scan`.

## JSON examples

List:

```bash
bob ref meet -l -f json -b /path/to/synthetic-vault
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"command":"ref meet","generated_at":"2026-11-24T15:00:00","meetings":[{"ref":"20261124#^cedar-review","path":"2026/20261124.md","block_id":"cedar-review","date":"2026-11-24","start":"14:00","end":"14:30","title":"Cedar Labs review","root":"job","root_is_inbox":false,"status":"*","event_id":"cedar-event","notes":null}]}
```

Preview:

```bash
bob ref meet -c -d 20261124#^cedar-review -b /path/to/synthetic-vault
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"command":"ref meet","generated_at":"2026-11-24T15:00:00","outcome":"would_file","meeting":{…},"match":{"method":"explicit","evidence":[…]},"filing":{"parent":"job","parent_kind":"project","title":"…","stem":"261124_cedar_labs_review","note":"ref/meetings/261124_cedar_labs_review.md","pdf":"lib/meetings/261124_cedar_labs_review.pdf","task":{"destination":"job","block_id":"…","status":"next"},"relation":{"label":"notes from","target":"[[2026/20261124#^cedar-review]]"}},"notes":{"sha256":"…","shape":"wispr_plain_text","outline":{"tldr":"…","next_steps":[],"sections":[]},"transcript":"omitted"},"warnings":[]}
```

Filed and queued receipts use `outcome: "filed"` or `"queued"` with one
copy of each key. Success overlays the committed note, PDF, task path, and
block id, plus `commit`, optional `commit_warning` / `sync_status` /
`push_pending`, `pages`, and warnings. Queued receipts add
`queued: {path, reason, detail}`, clear `filing.reroot`, and warn that
re-root was skipped. Errors use `error: {code, message, hint?}` with
`root`, `candidates`, `parent_candidates`, or `existing` where relevant.
Diagnostics go to stderr in JSON mode; stdout stays one envelope. See
[JSON outputs](reference/json-outputs.md#references-jobs-keep-plugins-and-status).

## Related

- [File meeting notes](guides/file-meeting-notes.md) — the CLI flow.
- [Reference library](ref.md) — reading state, identity, and rename guarantees.
- [Configuration](reference/configuration.md#meetings) — meetings keys.
- [JSON outputs](reference/json-outputs.md#references-jobs-keep-plugins-and-status) — versioned envelopes.

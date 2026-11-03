# Bob Gcal

`bob gcal` projects Google Calendar events onto today's daily note as Event
Tasks. This page covers one-time setup through
[gog](https://github.com/openclaw/gogcli) (≥ v0.44.0), the read-only event
list, and the vault synchronization that writes Event Tasks above today's
Pomodoros. `list` is the bare default.

Typical order: `bob gcal login` once, `bob gcal doctor` to confirm the
checklist, `bob gcal calendars` to copy calendar ids into config, then
`bob gcal sync` to write today's Events. Tests never
touch Google: set `BOB_GCAL_GOG` to a fake executable.

## Contents

- [Commands](#commands)
- [How it works](#how-it-works)
- [Setup](#setup)
- [Configuration](#configuration)
- [Login](#login)
- [Logout](#logout)
- [Doctor](#doctor)
- [Calendars](#calendars)
- [List](#list)
- [Sync](#sync)
- [Off-clock events](#off-clock-events)
- [Buffers](#buffers)
- [Description bullets](#description-bullets)
- [Reconciling Google-side changes](#reconciling-google-side-changes)
- [Where it runs](#where-it-runs)
- [Event inclusion](#event-inclusion)
- [Root selection](#root-selection)
- [Exit status](#exit-status)
- [JSON output](#json-output)
- [Security](#security)
- [Environment](#environment)
- [Known limits](#known-limits)

## Commands

```bash
bob gcal
bob gcal calendars [-f|--format table|json]
bob gcal doctor [-b|--bob-dir DIR] [-f|--format human|json]
bob gcal [list] [-a] [-b|--bob-dir DIR] [-D|--day DAY] [-f|--format table|json]
bob gcal login [-a|--account EMAIL] [-c|--credentials FILE] [-f|--format human|json] [-m|--manual]
bob gcal logout [-a|--account EMAIL] [-f|--format human|json] [-y|--force]
bob gcal sync [-b|--bob-dir DIR] [-D|--day DAY] [-d|--dry-run] [-f|--format human|json] [-q|--quiet]
```

Running `bob gcal` with no subcommand runs `bob gcal list`. The list is
read-only; `sync` writes the vault.

## How it works

Bob talks to Google only through `gog`. Common flags go before the subcommand
and never include tokens:

`--no-input --json --color never --readonly --client bob-gcal --account <email>`

The isolated `--client bob-gcal` token bucket means removing or re-consenting
Bob's Calendar grant never touches other gog tokens (Gmail included). Login
runs gog's remote two-step sign-in with captured `--no-input --json` input and
output. Client JSON goes to gog on stdin. Step 2 passes the single-use,
PKCE-bound redirect URL as `--auth-url`; the code is useless without gog's
stored verifier, and gog redeems it immediately. Bob owns the browser and
paste interaction, so gog never takes over the terminal.

`id: primary` in config matches a calendar whose `primary` flag is true or
whose id is the literal `primary`.

## Setup

1. Install [gog](https://github.com/openclaw/gogcli) ≥ v0.44.0 and put it on
   `PATH`.
2. Add a `gcal:` block (see [Configuration](#configuration)). A missing block
   uses defaults.
3. Run `bob gcal login`. It reuses a stored Bob client or a readable gog
   client, checks `~/Downloads` for a Desktop-app client, and guides you
   through Google Cloud setup if needed. You can provide a client directly
   with `-c PATH`. The [connect Google Calendar](guides/connect-google-calendar.md)
   guide explains each Cloud Console screen.
4. `bob gcal doctor` until every check is `ok` or an explained skip.
5. `bob gcal calendars` to copy ids into `gcal.calendars`.

## Configuration

`bob gcal` reads `~/.config/bob/config.yml` (override with `BOB_CONFIG_FILE`):

```yaml
gcal:
  account: you@example.com # optional when gog holds exactly one account
  writer: kellys-macbook-pro # optional; the one host label allowed to sync
  root: meet # default [root::]
  calendars: # default: [primary]
    - id: primary
    # - id: <calendar id from `bob gcal calendars`>
    #   root: body
  colors: {} # e.g. {tomato: job, sage: body}
  tentative: context # context | task
  description: bullets # bullets | off
  description_lines: 8 # most copied lines per Event Task, overflow line included; buffer reasons are extra
  timeout_secs: 60
```

- A missing or null `gcal:` block means defaults. Unknown keys are ignored.
- A wrong-typed value is a setup error (exit 2) that names the key, for
  example `gcal.timeout_secs`, and never breaks other config loaders.
- `account` must contain `@` when set. Commands resolve it in this order:

  | Source | When it wins |
  | --- | --- |
  | `bob gcal login -a EMAIL` | Explicit override (login only) |
  | `gcal.account` | Configured |
  | Sole `bob-gcal` account | The isolated bucket holds exactly one unique email |
  | Sole gog account | No Bob grant, and `gog auth list` holds exactly one unique email |
  | TTY picker | Login on a terminal when more than one email remains |
  | `account_ambiguous` | Other commands when more than one email remains |

  Unique emails are compared case-insensitively. A sole Gmail grant in gog's
  default bucket does not hide a `bob-gcal` Calendar account.
- `calendars` accepts a list of ids or a list of `{id, root?}` maps.
- `timeout_secs` must be an integer greater than 0. Default 60.
- `colors` maps Google event colour names or ids (`lavender`/`1` through
  `tomato`/`11`) to roots. The colour key is validated when config loads; the
  mapped root is checked against the vault for each event.
- `tentative` controls whether a busy Maybe event is context (default) or an
  Event Task (`task`).
- `description` controls whether sync copies each event's human description
  as bullets under its Event Task (`bullets`, default) or writes none
  (`off`, the kill switch).
- `description_lines` caps the copied lines per Event Task at an integer
  of at least 2 (default 8), overflow line included; buffer reasons are
  extra. A wrong type or a value below 2 is a setup error (exit 2) that
  names the key.
- The host label is the same slug vault-sync uses in commit subjects.
  `doctor` prints it. Unset `writer` means this host may write; a mismatch
  fails the writer check.

See [configuration](reference/configuration.md#gcal).

## Login

`bob gcal login` owns a three-step, read-only Calendar sign-in. Progress and
prompts go to stderr; with `-f json`, stdout contains only the report and
stderr shows only the interactive pieces (instructions, the link, and
prompts). Human mode leaves stdout empty. A healthy rerun is idempotent: it
still prints steps 1–3, `already connected`, the summary, and Next/Undo.

1. **Google client.** `-c FILE` loads and validates a Desktop-app client,
   replacing Bob's saved client. Otherwise Bob uses its saved client, copies a
   readable gog client whose secret is not keyring-only, then checks the newest
   valid `~/Downloads/client_secret*.json`. On a TTY, if none is available, Bob
   explains how to create one in Cloud Console and accepts a downloaded path or
   pasted JSON. A Downloads confirm accepts `Y`/`yes`/empty, rejects `n`/`no`,
   treats `q`/`quit`/EOF as cancelled, and re-prompts on anything else. Client
   secrets are sent to gog over stdin.
2. **Sign in.** `-a` overrides `gcal.account`; when neither selects an
   account, Bob uses the sole `bob-gcal` account, else the sole gog account, or
   asks on a TTY. A saved valid grant is reused. Otherwise Bob selects browser
   sign-in when `BOB_GCAL_BROWSER` is set, or when a desktop session is
   available (`DISPLAY` or `WAYLAND_DISPLAY` on Linux; always on macOS). SSH
   (`SSH_CONNECTION` or `SSH_TTY`) and headless sessions use the paste flow.
   `-m` always selects paste. Google grants only `calendar.readonly`. On paste,
   copy the full address from the expected `127.0.0.1:<port>` page and paste
   it into Bob. Both remote steps pass `--force-consent` and the bound
   `--redirect-uri`.
3. **Check.** Bob lists calendars and the configured sync set. `id: primary`
   and the primary calendar's real id both count as present. If the Calendar
   API is off, a TTY can turn it on and retry in place; without a TTY, or after
   `q`, the grant is kept and the hint says to enable the printed project URL
   then rerun `bob gcal login`. An expired or revoked grant triggers sign-in
   again.

SSH paste (reusing gog's Gmail client):

```text
$ bob gcal login -a you@example.com
Google Calendar · read-only · one-time setup

1/3  Google client  ✓ reusing gog's "default" client · project 123456789012
                    Bob keeps its own read-only sign-in; Gmail stays untouched.
2/3  Sign in        you@example.com · you're on SSH, so sign in from any browser:

https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=…&login_hint=you%40example.com&…

                    1. Open the link, choose you@example.com, and allow
                       "See and download any calendar you can access".
                    2. "Google hasn't verified this app"? It's your own client:
                       Advanced → Go to … (unsafe).
                    3. You land on a 127.0.0.1 page that can't load. That's expected:
                       copy that page's whole address.

                    Paste the address (? for help) › http://127.0.0.1:36015/oauth2/callback?state=…&code=…
                    ✓ signed in · read-only Calendar access granted
3/3  Check          ✓ 8 calendars · syncing 1: primary

✓ Connected. Bob can read, never change, you@example.com's calendars.

  Next  bob gcal sync -d     preview today's Events
        bob gcal calendars   choose which calendars to sync
  Undo  bob gcal logout      removes only Bob's Calendar grant
  Note  sync runs on the-mac (gcal.writer); run `bob gcal login` there too.
```

Fresh desktop, nothing set up yet:

```text
$ bob gcal login
Google Calendar · read-only · one-time setup

1/3  Google client  Bob needs an OAuth client from your own Google Cloud project.
                    One time, about 3 minutes, in your browser:

                    a. Turn on the Calendar API
https://console.cloud.google.com/apis/library/calendar-json.googleapis.com

                    b. Audience: External, then Publish app (In production;
                       unverified is fine for personal use)
https://console.cloud.google.com/auth/audience

                    c. Clients → Create client → Desktop app → Download JSON
https://console.cloud.google.com/auth/clients/create

                    Press Enter once the JSON is downloaded (Bob checks ~/Downloads), or paste its path ›
2/3  Sign in        you@example.com · opening your browser…
                    Allow "See and download any calendar you can access".
                    Browser didn't open? Use this link:

https://accounts.google.com/o/oauth2/auth?…

                    ✓ signed in · read-only Calendar access granted
3/3  Check          ✓ 3 calendars · syncing 1: primary
```

Healthy rerun:

```text
1/3  Google client  ✓ bob-gcal · project 123456789012
2/3  Sign in        ✓ already connected · you@example.com
3/3  Check          ✓ 8 calendars · syncing 1: primary

✓ Connected. Bob can read, never change, you@example.com's calendars.

  Next  bob gcal sync -d     preview today's Events
        bob gcal calendars   choose which calendars to sync
  Undo  bob gcal logout      removes only Bob's Calendar grant
```

`invalid_grant` can mean the grant expired or was revoked; Bob signs in again.
If this happens after about seven days, publish the OAuth consent screen in
Google Cloud Console (Testing grants expire after seven days). `consent_denied`
means Google refused access; rerun and allow it. `stale_link` means the pasted
or exchanged link is out of date; rerun for a fresh one. `cancelled` is a
deliberate stop (including paste `access_denied` then `q`). `needs_tty` means
the paste or setup prompt ran without a terminal.

## Logout

`bob gcal logout -y` removes the isolated `bob-gcal` refresh token only.
Gmail and other gog clients stay. Then revoke Bob's Calendar access at
`https://myaccount.google.com/permissions`. Without `-y`/`--force`, logout
asks for confirmation on a TTY and exits 2 on a pipe.

## Doctor

`bob gcal doctor` walks gog, client, account and scope, a live calendars call
(the token refresh), configured calendars, default-root validity and its type
suggestion, today's daily note, journal git rules, malformed journal lines,
the writer host, and the last sync (including its error, when the last run
failed).

A failing check prints `✗ <check>  <problem>` plus `  fix: <one command or
step>`. Exit 1 when any check fails; exit 0 when every check is `ok` or
`warn` (matching `bob gkeep doctor`).

JSON:

```json
{
  "schema_version": 1,
  "ok": true,
  "host": "kellys-macbook-pro",
  "checks": [
    {"id": "gog", "status": "ok", "summary": "v0.44.0 · /opt/homebrew/bin/gog", "fix": null}
  ]
}
```

Journal rules doctor expects:

```gitignore
!/.bob/gcal/*.jsonl
```

```gitattributes
.bob/gcal/*.jsonl merge=union
```

## Calendars

`bob gcal calendars` lists each calendar's id, name, access role, colour,
whether it is in the configured sync set, and its mapped root. Use the ids to
fill `gcal.calendars`. Default format is a table; `-f json` prints
`schema_version: 1` with `ok` and `calendars`.

## List

`bob gcal` and `bob gcal list` fetch the configured calendars for one local
day and show the complete snapshot. The list is a preview; it does not write
the vault or Google Calendar.

Before login, bare `bob gcal` explains that Calendar is not connected, points
to `bob gcal login`, and exits with setup status 2.

```bash
bob gcal
bob gcal -D tomorrow
bob gcal list -D 2026-10-30 -a -f json
```

`-D|--day` accepts `today`, `tomorrow`, `yesterday`, or `YYYY-MM-DD`. Relative
days use Bob's configured `timezone:` (the host timezone when unset) and honor
`BOB_NOW`. The day window runs from local midnight to the next local midnight,
so it is 23 or 25 hours when daylight saving time changes. Every configured
calendar is fetched with all pages; any transport failure, malformed event, or
leftover page token fails the whole list rather than presenting a partial
snapshot.

The table shows local time, title, RSVP, selected root, and the VAULT column:
what sync did, or would do, with the event — `task · MEET`, `task`,
`context`, `waiting for note`, or `removed by you`, each with
` · off-clock` appended for Google off-clock tasks
(`task · MEET · off-clock`). The TIME column appends the buffer label for
buffered tasks (`12:00-13:00 −30m +15m`, dimmed on a TTY); see
[Buffers](#buffers). The EVENT column appends a dim `≡ N` after the title
of each Event Task with N bullet lines sync would write, reasons
included; see
[Description bullets](#description-bullets). Omitted events keep their
omission reason. `-a` also shows omitted events. Counts include omitted events
even when their rows are hidden. The header carries the last sync: `synced
3 min ago on kellys-macbook-pro`, falling back to the newest journal record,
else `never synced`.

JSON has `schema_version`, `day`, `calendars`, `events`, `counts`,
`last_sync`, `past_open`, `warnings`, and `deprecations` fields. Each event
reports `time`, `title`, `answer`, `inclusion`, `reason`, `clock` (`"off"`,
`"on"`, or `null`, with conflicts normalized to `"on"`), `clock_source`
(`on_tag`|`off_tag`|`note`|`invalid_note`|`default`), `clock_note`
(the deciding note path or `null`), `off_clock`, deprecated `untracked`,
`buffer` (`{"before_minutes", "after_minutes"}`, or `null` when the row
carries no buffer), `description_lines` (bullet lines sync would write,
reasons included, 0 otherwise), `vault`, and `root.value`, `root.source`,
and `root.warnings`. `vault` appends ` · off-clock` for explicit off tags
and ` · off-clock via <stem>` for inherited clocks. Descriptions and
attendee details are omitted. A root may be null when no candidate is
valid; per-event `root.warnings` explain each rejected candidate and its
fix, while top-level `warnings` carry journal and Google clock-deprecation
notices. `past_open` lists the open Event Tasks still sitting
in the previous 7 daily notes (`day`, `title`); the table renders the same
as a footer.

## Sync

`bob gcal sync` reconciles one bounded, idempotent day — today by default,
`-D` for another day whose note already exists. Bob never creates daily notes;
when the note or its `## Pomodoros` section is missing, sync exits 0 with
`waiting for 2026/20261030.md`.

```bash
bob gcal sync
bob gcal sync -D tomorrow
bob gcal sync -d
bob gcal sync -q
```

The run fetches every configured calendar outside any lock, classifies each
event, chooses roots, and folds the per-host journal. The plan then inserts
new rows in time order (never reordering existing lines), adopts rows whose
`📅` URL is already in the note without a journal record (crash recovery),
tombstones journaled rows whose line is gone (never re-created), and files
each created Event Task under a past, running, or planned MEET Pomodoro in
the same write. A removed or moved link is never re-added. An unchanged
calendar can still write the Events stamp when the stamp is due (missing,
malformed, edited, misplaced, duplicated, later than now, or from an
earlier minute). The stamp shows the last sync run, and every run at a
new minute rewrites it, even when the calendar is unchanged. A
same-minute rerun writes nothing. A plan that is byte-identical after
that step writes nothing and appends nothing. Sync keeps a
blank line between the `## Events` heading and its first line, and adds the gap
to an existing section that lacks it, even when there are no calendar changes.

Every Event Task row this run creates (a new row, or a Maybe context row
upgraded in place) is linked by phase, on any day:

| Phase at sync time | Pomodoro | Link line | Row checkbox |
| --- | --- | --- | --- |
| Past: the buffered span ended (buffered end ≤ now) | A closed `- [x] (**SSSS-EEEE** [t:: Nm]) — MEET` entry for worked events or `(**SSSS-EEEE** [off:: Nm])` for an off-clock cluster | `🍅 [[#^id]]` | `[/]` |
| Current: buffered start ≤ now < buffered end, today's note only | The running MEET entry | `[[#^id]]` | `[*]` |
| Future: the buffered span has not started | The open MEET placeholder `- [ ] () — MEET` | `[[#^id]]` | `[*]` |

Every sync run also keeps each Pomodoro entry's dedicated links to same-note
Event Tasks in meeting-time order, matching the Events rows' bold spans. A
link moves with its child bullets; notes, non-Event Task links, and other
children stay in their existing positions. A hand reorder of Event Task links
is restored on the next sync.

Phases use the buffered span — what the event [occupies](#buffers) — while
each row keeps its meeting span: the bold `**HHMM-HHMM**`, its journal
hash, and Google ownership of the time never widen. Back-to-back or
overlapping past meetings share one closed MEET whose span is
the cluster (next buffered start ≤ cluster end; a `+15` lead-out bridges a
gap to the next meeting), split by clock verdict: a mixed run becomes two
closed MEETs that may overlap — one with `[t::]`, one with `[off::]`. All
current meetings share one
running MEET, and all future meetings share one placeholder. A created
running MEET is off-clock only when every current event is off-clock; mixed
current events stay worked. The run never
leaves a second open MEET: current meetings with no running Pomodoro reuse
an existing MEET placeholder if one is open, otherwise they create a running
MEET; future meetings go under the running MEET when that running entry is
named MEET, otherwise under the placeholder (created when missing). Sync
never creates a running entry while any open timed Pomodoro exists; those
current meetings go under the placeholder with a warning. Sync never starts,
closes, retimes, renames, or moves an existing Pomodoro: it only creates
entries, adds links, and reorders Event Task links within an entry. Existing
unlinked rows still earn an open-MEET link only on today's note, and only
when the event has not ended.

A past day (`-D yesterday`) gains closed MEET entries and `[/]` rows. A
future day (`-D tomorrow`) gains a MEET placeholder and `[*]` rows; task
status hooks rule R9 may clear those `[*]` rows to `[ ]` until that day
becomes today, when the MEET link raises them again.

The write takes the vault lock (30 seconds), re-reads and re-plans, then
writes through the preimage-checked staged writer, all-or-nothing, retrying
once on a concurrent edit before failing. The journal (`.bob/gcal/<host>.jsonl`)
appends only after the note write succeeds, and host-local status
(`$XDG_STATE_HOME/bob/gcal/status.json`) records the run for `list` and
`doctor`. Commits are left to vault-sync, as with capture. `-d` prints the
exact unified diff of the plan and writes nothing; the diff shows the exact
description bullets, and is the only place description text appears in
command output. `-q` suppresses success chatter on
success, no-op, or waiting, while clock-deprecation warnings stay visible
on stderr. Human dry-run diffs also print those warnings on stderr; JSON
mode carries `warnings` and `deprecations` with no duplicate stderr
diagnostics.

`sync -f json` reports `schema_version`, `ok`, `day`, `path`,
`status` (`written`, `unchanged`, `waiting`, or `dry_run`), `rows`
(`action`, `kind`, `time`, `title`, `root`, `source`, `off_clock`, deprecated
`untracked`, `buffer` as `{"before_minutes", "after_minutes"}` or
`null`, and `description_lines`, the bullet lines this run wrote for the
row, reasons included, 0 otherwise), `pomodoro`
(`entry_line`, `created`, `links_added`, `links_sorted`, `entries`),
`warnings`, `deprecations`, and `stamp`
(`account`, `synced_at`, `refreshed`, or `null` when there is no Events
section and for `waiting`). Human rows append the buffer label to the time
(`~ task 12:00–13:00 −30m +15m  Lunch …`), and a dim `≡ N` when the run
wrote N [description bullets](#description-bullets) for the row, reasons
included
(`+ task  08:00–08:15  Swings!  ● love  via @love ≡ 1`). MEET lines show buffered
spans. `pomodoro.entries` lists each MEET this run
created or appended links into (`state` `closed`/`running`/`planned`,
1-based `line`, `time` as `HHMM-HHMM` or null, `created`, `links`) in
ledger order. `entry_line` and `created` describe the first open entry in
that list (`null` / `false` when the run linked only under closed entries).
`links_added` counts every link, closed entries included. `links_sorted` counts
Event Task link lines moved into meeting-time order. A sort-only human summary
looks like `✓ Events unchanged · Pomodoros ↕2 links sorted`; other Pomodoro
summaries append `, ↕2 sorted` when links moved.

### Events stamp

Every reconciled `## Events` section carries one Bob-owned italic line
immediately after the heading gap:

```markdown
## Events

*↻ synced 14:05 · [you@example.com](https://calendar.google.com/calendar/r/day/2026/10/30?authuser=you%40example.com)*
- [x] #task **0800-0830** Product interview [📅](…) [📹](…) [root:: job] ^product-interview
```

The source is `*↻ synced <when> · [<account>](<day-url>)*`. `<when>` is `HH:MM`
in Bob's timezone when the run happened on the note's own day, otherwise
`Ddd YYYY-MM-DD HH:MM` (for example `synced Thu 2026-10-29 21:40` on Friday's
note after `bob gcal sync -D tomorrow`). The account is a deep link that opens
that day in Google Calendar; the human report echoes `↻ synced 14:05 ·
you@example.com` with no URL. Sync never creates an Events section just to hold
a stamp. A hand-written section that sync reconciles gets one.

The stamp shows the last sync run. Every run at a new minute rewrites it,
even when the calendar is unchanged. A same-minute rerun writes nothing.
Sync also rewrites it when rows, links, the heading gap, or the stamp
itself changed, and when the existing line is missing, malformed, edited,
misplaced, duplicated, or later than now. Each run at a new minute
rewrites the daily note, and vault-sync commits it. Ledger Tools never
paints the line stale. Bob owns the line: an edit is replaced and a
deleted line returns on the next sync. Tombstones and "propose once" do
not apply.

A day whose stamp names a different Google account than this run uses is
refused (`account_changed`, exit 2) so a switch cannot cancel the old
account's untouched meetings. Delete the `↻` line to hand the day to the new
account, or set `gcal.account` to the stamp's account.

### Who owns each part of a row

| Part | Owner | Sync behavior |
| --- | --- | --- |
| Checkbox | You | Never changed, except cancelling an untouched task (see below) |
| Time span, title, badges | Google | Updated in place only while untouched; your edits win and are reported once |
| `#off` / `#on` (deprecated Google aliases `#offclock`, `#off-clock`, `#untracked`) | Google until you edit it | Follows Google's explicit tag while untouched; your add/remove wins and is reported once. A flip after the session closed is left for a hand edit and reported once as `kept_off_clock` |
| `[buffer::]` | Google until you edit it | Follows Google's description directives while untouched; your add/remove wins and is reported once. No field when the event has no buffer |
| Description bullets (buffer reasons included) | Google until you edit them | Rewritten in place while they still match what Bob last wrote; your edit, nested note, or deletion makes them yours, reported once only if Google later changes |
| `[root::]` | Google until you edit it | Follows the chosen root (not series memory) until you edit it, and adds `[type:: MEET]` when needed. Your edits win, are reported once when Google later routes elsewhere, and teach the series |
| `[type::]`, block ID | Bob, once | `[type:: MEET]` is added on a root reroute only when the line has none; an existing `[type::]` and the block ID are never rewritten |
| Other child bullets (your notes) | You | Never touched |
| MEET Task Link | Bob, once | Never re-added; kept in Event Task time order within its entry, so a hand reorder is undone; removed only by a cancel |
| MEET Pomodoro entries | Bob, once | Created only in the write that adds their first link; never started, closed, retimed, renamed, or moved afterwards |
| The whole row | You | Deleted or moved away: tombstoned, never re-added |
| Events stamp | Bob | Re-rendered when the note is written or the stamp is due; a deleted or edited line is restored |

In Obsidian, each MEET Task Link to an Event Task wears that event's time pill
([Event link times](event-link-times.md)).

Badges are plain title-free links (`[📅](url)`), so link-hint plugins such as Jump to link open them correctly, and rows written by older Bob versions migrate on that day's next sync while untouched.

"Untouched" means not done, not In Progress, never linked under a closed
Pomodoro, and with no 🍅 history. A worked task is never cancelled; it is
left alone and reported once. A Past row is worked from birth (`[/]` and a
closed-entry link), so a later Google decline, delete, or move leaves it
alone and reports it once. `[/]` rows count in the Pending lane and show in
`list`'s `past_open` footer until they are completed or cancelled. MEET
placeholders and running MEET entries are created only in the same write
that adds their first link, so sync never leaves a second open MEET.

<a id="off-clock-events"></a><a id="untracked-events"></a>

## Off-clock events

Tag a Google Calendar event `#off` (lunch, school pickup, the gym) so it
still lands in the day as an Event Task with a Pomodoro Task Link, but its
session stays on the calendar and off the Worked clock. `#on` explicitly
forces on-clock and wins over any off spelling. The closed MEET uses
`[off:: Nm]` with the event's real duration; Worked stats and
`bob pomodoro --worked` leave those minutes out, while homes can attribute
them. The spellings `#offclock`, `#off-clock`, and legacy `#untracked`
remain accepted forever but are deprecated: sync and list report them and
point at `#off`.

Google titles use whole whitespace tokens, ASCII case-insensitive, with
trailing `.,;:!?` stripped: `Lunch #OFFCLOCK!` marks off-clock while
`#office`, `#one`, and `#off/x` never do. Description parsing keeps its
existing HTML/plain normalization and whitespace/comma directive
tokenization: every token on a line must be a directive. A directive line is
one whose every token (split on whitespace or commas) is a directive:
`@route`, `#off`, `#on`, a deprecated off alias, `-N`/`-Nm` (ASCII or
typographic minus), or `+M`/`+Mm`. Directives combine on one line or spread
across several. A plain-text list marker is stripped before checking the
tokens, so `- #off` is a directive line, like an HTML `<li>` after
normalization. HTML from Google's editor is normalized first (`<br>`,
`<p>`, `<div>`, `<li>`, headings, list containers, table rows, and `<hr>`
become line breaks, other tags are stripped, common entities are decoded).
With a resolvable `love` root, `@love<br>#off -30` sets `[root:: love]`,
marks the Event Task off-clock, and adds a 30-minute lead-in. A line with
any other token — a phone number, a 4-digit value, a sentence — is ignored
whole, so `Bring shoes #offclock` stays prose and never marks the clock.
The one exception is a buffer reason line (see
[Buffer reasons](#buffer-reasons)).
Sync strips the title clock token from the displayed title before reading a
title-ending `@route`, so `Lunch @love #off` gets the same root and
displays as `Lunch`.

Explicit tags and the effective clock are separate values. Rendering and
badge hashes depend only on Google's explicit tag: sync writes ` #off`,
` #on`, or no tag at the end of the badge run, before `[root::]`, and an
untagged `Lunch @love` that resolves off-clock through its root gains no
inherited tag. `#on` wins conflicts; otherwise off wins, then the nearest
area/project `offclock:` on the shared PT13 root walk decides, then the
default is on-clock. See the vault-wide precedence in
[pomodoro homes](pomodoro-homes.md). `Lunch @love #on` can force a
love-rooted lunch back onto the Worked clock.

```markdown
- [*] #task **1200-1300** Lunch with Kelly [📅](…)
  #off [root:: love] [type:: MEET] ^lunch-with-kelly
```

The tag is Google-owned exactly like the badges: updated in place while the
row is untouched, and your vault edit (adding or deleting it) wins and is
reported once. Context rows strip the token and carry no tag; a Maybe context
row upgraded to a task renders the tag. A tag flip after the session closed
is reported once and left for a hand edit — sync never retimes existing MEET
entries. Untouched legacy `#offclock` spans migrate to `#off` without
changing row IDs, roots, children, or ledger stamps. Alias normalization is
not a clock flip, including on a worked row.

Sync computes MEET clocks from the line left after this run's merge,
including retained vault tags and roots. List previews the same event
through Google's chosen root instead: an untagged love-rooted lunch creates
an `[off::]` past cluster and running MEET with no tag on its row, while
`#on` keeps it worked. This intentional difference is covered by the
`clock`, `clock_source`, and `clock_note` fields.

`list` shows `task · MEET · off-clock` in VAULT for explicit off tags and
`task · MEET · off-clock via <stem>` for inherited clocks; JSON carries
`clock`, `clock_source`, `clock_note`, `off_clock`, and deprecated
`untracked`. `sync -f json` rows carry the effective `off_clock` and
deprecated `untracked` for the retained row. Both commands add top-level
`warnings` and `deprecations`: one warning per deprecated occurrence per
run (unchanged and dry-run included) and one structured record per distinct
deprecated alias with `calendar_id`, `event_id`, `title`, `token`,
`where` (`title`|`description`|`both`), `replacement` (`#off`),
`drop_suggested`, and `on_wins`. Google stays read-only: deprecations never
create journal writes and never make an otherwise unchanged plan changed.
A canonical `#off` plus `#on` warns with `clock_tag_conflict` and states
that `#on` wins.

An ended `#off` Event Task nobody started settles into a closed
`[off:: Nm]` MEET on the next `bob task reconcile` (see
[Settled off-clock events](task-status-hooks.md#settled-off-clock-events)).
Cancel the row before it ends to keep it off the record.

## Buffers

Write `-30` and/or `+15` as [directive lines](#off-clock-events) in a
Google Calendar event's description to reserve 30 minutes before and 15
minutes after the event (travel, prep, wind-down). Each side takes 1–3
digits with an optional `m` suffix and an ASCII or typographic minus, is
capped at 720 (12h), and the first value per side wins. The tokens combine
with `@route` and the clock tags (`#off`, `#on`, deprecated `#offclock`,
`#off-clock`, `#untracked`) on one line or spread over several, so
`@love #off -30`
routes, marks off-clock, and buffers at once. Numbers in
titles are never buffers: `Q4 +1` and `Room -2` stay titles. Bob reads
buffers from the description only.

Bob treats the event as occupying the **buffered span** — `start − before`
to `end + after`, with a pre-buffer stopping at midnight — while the
meeting keeps its real time:

| Uses the buffered span | Keeps the meeting span |
| --- | --- |
| Sync phases (past/current/future) | Timed close classification (`ended_inside`/`running`/…) |
| The MEET clusters and spans sync writes | The bold span text, its journal hash, and Google ownership of the time |
| The TIME label and `buffer` JSON in `list` and `sync` output | Row titles, `#off`/`#on`, roots |

Sync writes one Google-owned field at the end of the badge run, after
`#off`/`#on` and before `[root::]`:

```markdown
- [*] #task **1200-1300** Lunch with Kelly [📅](…) #off
  [buffer:: -30m +15m] [root:: love] [type:: MEET] ^lunch-with-kelly
```

A side that is zero is omitted (`[buffer:: -30m]`, `[buffer:: +15m]`),
and an event without buffers has no field. `list` shows
`12:00-13:00 −30m +15m`, and JSON events carry
`buffer: {"before_minutes": 30, "after_minutes": 15}`. Hand-written Event
Tasks may carry the field too: capture and the `capture-pomodoros --tasks`
agenda read the buffer from the vault field, while sync reads it from the
Google description.

The field is Google-owned exactly like the badges: it updates in place
while the row is untouched, and a vault edit wins and is reported once.
Rows synced before this feature migrate on the next sync. Context rows
(Maybe/free) carry no buffer; a Maybe row upgraded to a task renders it.

### Buffer reasons

A description line like `-30 Drive to Kelly's` or `+15 Drive home` sets
that side's buffer and writes one labelled reason bullet at the top of
the Event Task's Google-owned description bullets:

```markdown
- [*] #task **1200-1300** Lunch with Kelly [📅](…) #off [buffer:: -30m +15m] [root::
  love] [type:: MEET] ^lunch-with-kelly
  - ↤ *30m before* · Drive to Kelly's
  - ↦ *15m after* · Drive home
  - Bring the gift card
```

A reason line is `<sign><1–3 digits>[m] <separator> <reason>`: `-` (or
`−`/`–`) for before, `+` for after, an optional `m`, then `:`, `,`, or
whitespace, optionally followed by `—`/`–`/`-`/`:` plus whitespace, then
the reason. `-30 Drive`, `-30m: Drive`, `−30 — Drive`, and
`+15, Walk home` are reasons; `+1 555-1212`, `-30 +15 Drive`,
`-1234 Drive`, `-30Drive`, and `-30 "Drive"` stay prose. The token and
separator must be plain text (bold or italic is fine, a link label is
not), and the reason must start with a letter or a non-ASCII character
other than a dash. `@route`, `#off`, and `#on` inside the reason are
text, not directives — keep them on their own line.

The first value per side wins, for directive and reason lines together:
a reason attaches when its line sets that side or repeats the same value
with no reason yet, and a disagreeing line (or a second reason for the
side) stays an ordinary bullet. One reason line carries one buffer.

## Description bullets

Whatever you write in a Google event's description, except Bob's directive
lines and video-call boilerplate, appears as bullets at the top of that
meeting's Event Task, and keeps following Google until you edit those
bullets.

Copied: the human lines of the description — plain paragraphs, list items,
headings, table rows, and links. Dropped: [directive
lines](#off-clock-events) (they drive `@route`, `#off`/`#on`, and buffers
instead), recognized conferencing blocks (Meet, Zoom, Teams, Webex),
separators, blank lines, and lines that only repeat the `📹` or `📍` target.
A Teams or Zoom invite whose description is only vendor boilerplate writes
no bullets when its join link is already on the row's `📹` badge. Without
that badge, Bob keeps one linked `Join … meeting` bullet so the invite's
join URL remains available. `Bring shoes #offclock` stays prose: the
directive grammar does not change, so a line with any non-directive token
is never a directive.

Buffer reasons lead the block: a `↤ *Nm before*` bullet, then a
`↦ *Nm after*` bullet, then the copied description bullets in order, then
the overflow line. Reasons never count toward `description_lines` and are
never truncated; the cap applies to copied lines only.

The shape is flat: first children of the Event Task, with one nesting level
for a list under its lead-in (`Agenda:`) and native `1.` items for ordered
lists. Managed first children (CANCEL LOG, Depends-On, LINKS, Schedule and
Work logs) stay above the bullets; your own notes go below them. Every
bullet shares the row's existing child indentation.

Description text is inert data: organizer prose can never mint a tag, task,
drawer, checkbox, embed, task link, or managed log. A few examples:

| Description line | Bullet written |
| --- | --- |
| ``Run `$= dv.el("b", 1)` now`` | ``Run \`$= dv.el("b", 1)\` now`` |
| `- [ ] RSVP #task by Friday` | `RSVP \#task by Friday` |
| `TBD` | `**TBD**` |
| `Bring 🍅 salad` | `Bring &#127813; salad` (still shows 🍅) |

At most `gcal.description_lines` copied lines per Event Task (default 8,
minimum 2, overflow line included); buffer reasons are extra. The overflow
line reads *…N more lines
in [Google Calendar](https://calendar.google.com/event?eid=abc)* and links
the event. A description above 64 KiB
writes the reasons, then one fallback bullet linking Google Calendar instead.
`gcal.description: off` is the kill switch: sync writes no new bullets,
reasons included, leaves existing ones alone, and carries their journal
records forward.

Bullets land with every Event Task sync creates, and with every Maybe→Yes
upgrade (directly under the upgraded row, above your notes there). Context
rows never get bullets. Reasons follow the same ownership as the block:
while the block still matches what Bob last wrote, a later Google edit
rewrites it in place, and clearing the description removes it. Your edit,
a nested note under a Google bullet, or deleting the block makes the
bullets yours: they are never re-added, and a `kept_description` warning
fires once only if Google's description later changes. Rows synced before
this feature get their bullets on the next sync of their day; matching
existing content is adopted, never duplicated.
Human `sync` rows end with a dim `≡ N` when the run wrote N bullet
lines, reasons included; `sync -d` shows the exact bullets in its diff —
the only place description text appears in command output. JSON carries
only `description_lines` (reasons included), never description text.

## Reconciling Google-side changes

Each sync compares every journaled row that is still present against a fresh
render of its event:

- A span (time, title, badges) whose current text still matches what Bob
  last wrote is replaced in place. A span that differs from both is your
  edit: it is kept and reported once as a `kept_edit` warning. A badge-format
  change rides the same in-place span update. A time
  change re-slots the row together with its child block to keep time
  order. The checkbox and block ID are never rewritten. An unedited
  `[root::]` follows Google's chosen root (never series memory) and
  gains `[type:: MEET]` when the line has none; an existing `[type::]`
  stays. A `[root::]` you edited is kept and reported once as a
  `kept_root` warning when Google later routes elsewhere.
- Description bullets follow Google until you edit them. A block that
  still matches what Bob last wrote is rewritten in place (or removed
  when Google clears the description). Your edit, a nested note, or a
  deletion makes them yours: they are never re-added, and a
  `kept_description` warning fires once only if Google's description
  later changes. Rows synced before this feature get their bullets on
  the next sync of their day; matching existing content is adopted,
  never duplicated.
- A Maybe context row whose RSVP went Yes upgrades in place to an Event
  Task and is filed under a MEET by phase. A Yes task row whose RSVP went
  Maybe stays a task and is reported once.
- A journaled occurrence that leaves the snapshot is looked up with
  `gog calendar event`. A 404/410, a `status: cancelled`, a start on
  another day, or a declined RSVP cancels an untouched task (checkbox to
  `[-]`, `[cancelled:: today]`, a created `❌ **CANCEL LOG**` first
  child, MEET link dropped) or removes an untouched context row. A worked
  task is left alone and reported once. A failed lookup makes the snapshot
  incomplete: nothing is written. Cancel reasons read `Declined in Google
  Calendar`, `Deleted in Google Calendar`, or `Moved to Fri 2026-10-31 in
  Google Calendar`. A cancel that empties an untimed MEET entry removes
  the entry.
- One `[root::]` edit on an occurrence of a recurring event teaches the
  series: new rows in that series use the learned root (still validated),
  reported as `learned from your 2026-10-29 edit`. A later edit, including
  a change back, re-teaches the series. Series splits are a cancel plus
  an insert, never a fuzzy match.
- Missed meetings are never auto-cancelled. `list` ends with the open
  Event Tasks still sitting in the previous 7 daily notes, for example
  `2 past meetings still open: Thu Dentist, Tue Weekly 1:1 · close or
  cancel them in their notes`.

## Where it runs

The Mac is the single writer: Templater creates daily notes there, the
browser login and Keychain token are local, and vault-sync's remote-wins
conflict policy would quarantine live ledger edits if another host wrote
today's note. Set `gcal.writer` to the Mac's host label; other hosts may
still run `list`, `calendars`, `doctor`, and `sync -d` after their own login.

Two triggers run sync on the writer:

- On demand (`bob gcal sync`, including via `bob gcal sync -D tomorrow`).
- Bob Ledger Tools runs `bob gcal sync -q -f json` once when today's daily
  note is created in Obsidian
  ([Calendar Events](reference/plugins/bob-ledger-tools.md#calendar-events)).

There is no periodic sync, so invitations accepted later in the day land
on the next run.

## Event inclusion

The first matching rule decides how an event appears:

| Event | List result |
| --- | --- |
| Event type other than `default` or `fromGmail`, or cancelled | Omitted |
| Declined or awaiting a reply | Omitted (`-a` shows the reason) |
| Busy, timed, under 24 hours, starts on this local day, and accepted, organized by you, or has no guests | Event Task |
| Same timed event with a Maybe RSVP | Context; `gcal.tentative: task` promotes it to an Event Task |
| All-day, multi-day, or free event with a Yes or Maybe RSVP | Context |

Bob reads your answer from the attendee whose `self` field is true. An event
organized by you or with no guests counts as Yes. A timed event under 24 hours
belongs only to the local date it starts, even if it crosses midnight. All-day
events use Google's exclusive end date and appear on every date they cover.
Timed multi-day events appear as context on each date they overlap.

## Root selection

For Event Tasks, Bob tries these candidates in order and writes the first valid
one: the series memory of your earlier `[root::]` edit on another occurrence
of the same recurring event, a title-ending `@route` or a description
directive line (combinable and HTML-safe, so `@love #off -30` on one
line routes), `gcal.colors` by event colour, the calendar's
`root`, then `gcal.root` (default `meet`). A root must resolve relative to
the daily note, be an area at any depth or a root project, and must not be
`@unfiled`.

Invalid candidates fall through to the next source with a warning that includes
the R11 reason and fix. If a title route resolves, Bob removes the final token
from the displayed title. Clock tags are stripped before the route is
read, so `Lunch @love #off` routes to `love`. An unknown route stays in the title and is reported.
If the default root also fails, list shows no root and `doctor` reports the
invalid default. Root sources are reported as `learned from your 2026-10-29
edit`, `@body`, `colour tomato`, `calendar Health`, or `default`. The VAULT
column reports the same roots beside the note.

Adding, changing, or removing a token, or a colour, calendar, or default
change, reroutes unedited existing rows on the next sync. Sync has no
periodic trigger, so run `bob gcal sync` or wait for the next daily-note
sync.

## Exit status

- `0` — success, including "already connected", a doctor run with only
  warnings, an unchanged sync, and a sync waiting for the daily note.
- `1` — runtime failure: gog, network, timeout, token, a doctor check
  that failed, or a daily-note write that lost a concurrent-edit race.
- `2` — usage or setup error: clap usage errors, bad `gcal:` config, missing
  gog, gog older than v0.44.0, no client, not logged in, sync on a host
  other than `gcal.writer`, or `account_changed` when the day's Events stamp
  names a different Google account than this run.

Errors print `bob gcal <cmd>: <message>` to stderr (a red `error` prefix on a
TTY) with an optional dim `  hint: …` line. In JSON mode, failures print
`{"schema_version":1,"ok":false,"error":{"kind":…,"message":…,"hint":…}}` to
stdout.

## JSON output

Every JSON document carries `schema_version: 1`. JSON never includes tokens,
descriptions, or attendees.

`login -f json`: `ok`, `already_connected`, `account`, `client`, `calendars`,
`syncing`, `syncing_ids`, `sign_in` (`already_connected`, `browser`, or
`paste`), `client_source` (`stored`, `file`, `gog_client`, `downloads`, or
`pasted`), `client_project` (string or null), and `reconnected` (boolean).

`logout -f json`: `ok`, `account`, `client`, `revoke_url`.

`doctor -f json`: `ok`, `host`, `checks` (`id`, `status`, `summary`, `fix`),
including the `last_sync` check.

`calendars -f json`: `ok`, `calendars` (`id`, `name`, `access_role`, `color`,
`primary`, `synced`, `root`).

`list -f json`: `day`, configured `calendars`, classified `events`,
`counts`, `last_sync`, `past_open`, `warnings`, and `deprecations`. `-a`
adds omitted event rows. Each event contains no provider description or
attendee list, and carries `clock`, `clock_source`, `clock_note`,
`off_clock`, deprecated `untracked`, and the `vault` state.

`sync -f json`: `ok`, `day`, `path`, `status`, `rows`, `pomodoro`,
`warnings`, `deprecations`, and `stamp`. `stamp` is
`{account, synced_at, refreshed}` or `null`. `pomodoro` carries
`entry_line`, `created`, `links_added`, `links_sorted`, and
`entries` as in [Sync](#sync). Row actions add `updated`, `cancelled`,
`removed` (context), `upgraded`, and `learned` to the sync-phase `added`,
`adopted`, and `removed` (task) actions; lifecycle notices also arrive as
`warnings`. `deprecations` carry one record per distinct deprecated Google
clock alias per occurrence (see [Off-clock events](#off-clock-events)).

## Security

Tokens never appear in argv, logs, or JSON. Client JSON is piped to
`gog auth credentials set -` on stdin. The only credential-like value in argv
is the single-use, PKCE-bound authorization code inside step 2's `--auth-url`;
it cannot be replayed without gog's stored verifier, and gog redeems it
immediately. The `bob-gcal` client is a separate bucket from Gmail. Tests use
`you@example.com` and never a live cookie.

## Environment

| Variable | Effect |
| --- | --- |
| `BOB_DIR` | Vault root; defaults to `~/bob`. |
| `BOB_CONFIG_FILE` | Config file holding the `gcal:` section. |
| `BOB_GCAL_GOG` | Executable replacing `gog` on `PATH`. Tests point this at a fake. |
| `BOB_GCAL_ASSUME_TTY` | `1` or `true` treats stdin as a TTY for login setup, paste prompts, denial retries, API-disabled retries, and logout confirm (test hook). |
| `BOB_NOW` | Local datetime override for today and relative list-day selection. |
| `SSH_CONNECTION`, `SSH_TTY` | Select the paste-URL sign-in flow when `BOB_GCAL_BROWSER` is unset. |
| `DISPLAY`, `WAYLAND_DISPLAY` | Non-empty values count as a Linux desktop session and select browser sign-in. macOS always counts as a desktop session. |
| `BOB_GCAL_BROWSER` | Browser executable; when non-empty, selects browser sign-in even over SSH. |

## Known limits

- One writer: `gcal.writer` pins the single host allowed to sync.
- Description bullets are a flat first-child block. A nested note under a
  Google bullet, or deleting the block, makes them yours; Bob never
  re-adds them. Sibling notes and managed lines above or below do not.
- Google-side changes apply only to spans you have not edited; your edits
  are kept and reported once.
- Google forbids Calendar scopes on the device-code flow; over SSH, login
  switches to the paste flow automatically.
- Consent screens left in Testing expire in 7 days; publish them.
- A closed or running MEET built from calendar spans may overlap a session
  already logged; sync does not clip it.
- Past clusters split by clock verdict, so two closed MEETs may overlap; that
  is truthful. One running entry is allowed, so mixed current events stay
  worked.
- An off-clock tag flip after the session closed is left for a hand edit and
  reported once as `kept_off_clock`.
- Buffers stop at midnight: a pre-buffer never crosses into the previous
  day, and the total stays below 24h. The first value per side wins. A
  description you cannot edit cannot take a Google-owned buffer: a
  hand-added `[buffer::]` is your edit — kept and reported once.
- Someone else's line such as `+1 to that` reads as a 1-minute
  after-buffer with a reason; the first line per side wins.
- R9 may clear `[*]` Event Tasks on a future day's note until that day is
  today.
- An off-clock event ending after the last reconcile before midnight does not
  settle.

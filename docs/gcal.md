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
`context`, `waiting for note`, or `removed by you`. Omitted events keep their
omission reason. `-a` also shows omitted events. Counts include omitted events
even when their rows are hidden. The header carries the last sync: `synced
3 min ago on kellys-macbook-pro`, falling back to the newest journal record,
else `never synced`.

JSON has `schema_version`, `day`, `calendars`, `events`, `counts`,
`last_sync`, and `past_open` fields. Each event reports `time`, `title`,
`answer`, `inclusion`, `reason`, `vault`, and `root.value`, `root.source`,
and `root.warnings`. Descriptions and attendee details are omitted. A root
may be null when no candidate is valid; warnings explain each rejected
candidate and its fix. `past_open` lists the open Event Tasks still sitting
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
tombstones journaled rows whose line is gone (never re-created), and appends
each eligible MEET link once. A removed or moved link is never re-added. A
byte-identical plan writes nothing and appends nothing.

The write takes the vault lock (30 seconds), re-reads and re-plans, then
writes through the preimage-checked staged writer, all-or-nothing, retrying
once on a concurrent edit before failing. The journal (`.bob/gcal/<host>.jsonl`)
appends only after the note write succeeds, and host-local status
(`$XDG_STATE_HOME/bob/gcal/status.json`) records the run for `list` and
`doctor`. Commits are left to vault-sync, as with capture. Past days get rows
only; only today gains MEET links. `-d` prints the exact unified diff of the
plan and writes nothing. `-q` prints nothing on success, no-op, or waiting.

`sync -f json` reports `schema_version`, `ok`, `day`, `path`,
`status` (`written`, `unchanged`, `waiting`, or `dry_run`), `rows`
(`action`, `kind`, `time`, `title`, `root`, `source`), `pomodoro`
(`entry_line`, `created`, `links_added`), and `warnings`.

### Who owns each part of a row

| Part | Owner | Sync behavior |
| --- | --- | --- |
| Checkbox | You | Never changed, except cancelling an untouched task (see below) |
| Time span, title, badges | Google | Updated in place only while untouched; your edits win and are reported once |
| `[root::]` | Precedence | Written once; your edits win, are never rewritten, and teach the series |
| `[type::]`, block ID | Bob, once | Never rewritten |
| Child bullets (meeting notes) | You | Never touched |
| MEET Task Link | Bob, once | Never re-added or moved; removed only by a cancel |
| The whole row | You | Deleted or moved away: tombstoned, never re-added |

"Untouched" means not done, not In Progress, never linked under a closed
Pomodoro, and with no 🍅 history. A worked task is never cancelled; it is
left alone and reported once. MEET placeholders are created only in the
same write that adds their first link, so sync never leaves a second open
MEET.

## Reconciling Google-side changes

Each sync compares every journaled row that is still present against a fresh
render of its event:

- A span (time, title, badges) whose current text still matches what Bob
  last wrote is replaced in place. A span that differs from both is your
  edit: it is kept and reported once as a `kept_edit` warning. A time
  change re-slots the row together with its child block to keep time
  order. The checkbox, `[root::]`, `[type::]`, and block ID are never
  rewritten.
- A Maybe context row whose RSVP went Yes upgrades in place to an Event
  Task (with a MEET link when the meeting has not ended yet). A Yes task
  row whose RSVP went Maybe stays a task and is reported once.
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

Two triggers run sync on the writer, and every run is idempotent:

- A Mac LaunchAgent, installed beside the vault-sync agent, runs
  `bob gcal sync -q` at load and every 5 minutes, so invitations accepted
  later in the day still land; see
  [background triggers](vault-git-sync.md#background-triggers).
- Bob Ledger Tools runs `bob gcal sync -q -f json` once when today's daily
  note is created in Obsidian
  ([Calendar Events](reference/plugins/bob-ledger-tools.md#calendar-events)).

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
of the same recurring event, a title-ending `@route` or description line
containing only `@route`, `gcal.colors` by event colour, the calendar's
`root`, then `gcal.root` (default `meet`). A root must resolve relative to
the daily note, be an area at any depth or a root project, and must not be
`@unfiled`.

Invalid candidates fall through to the next source with a warning that includes
the R11 reason and fix. If a title route resolves, Bob removes the final token
from the displayed title. An unknown route stays in the title and is reported.
If the default root also fails, list shows no root and `doctor` reports the
invalid default. Root sources are reported as `learned from your 2026-10-29
edit`, `@body`, `colour tomato`, `calendar Health`, or `default`. The VAULT
column reports the same roots beside the note.

## Exit status

- `0` — success, including "already connected", a doctor run with only
  warnings, an unchanged sync, and a sync waiting for the daily note.
- `1` — runtime failure: gog, network, timeout, token, a doctor check
  that failed, or a daily-note write that lost a concurrent-edit race.
- `2` — usage or setup error: clap usage errors, bad `gcal:` config, missing
  gog, gog older than v0.44.0, no client, not logged in, or sync on a host
  other than `gcal.writer`.

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
`counts`, and `last_sync`. `-a` adds omitted event rows. Each event contains
no provider description or attendee list, and carries the `vault` state.

`sync -f json`: `ok`, `day`, `path`, `status`, `rows`, `pomodoro`, and
`warnings`. Row actions add `updated`, `cancelled`, `removed` (context),
`upgraded`, and `learned` to the sync-phase `added`, `adopted`, and `removed`
(task) actions; lifecycle notices also arrive as `warnings`.

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
- Google-side changes apply only to spans you have not edited; your edits
  are kept and reported once.
- Google forbids Calendar scopes on the device-code flow; over SSH, login
  switches to the paste flow automatically.
- Consent screens left in Testing expire in 7 days; publish them.

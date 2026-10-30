# Bob Gcal

`bob gcal` projects Google Calendar events onto today's daily note as Event
Tasks. This page covers the read-only event list and one-time setup through
[gog](https://github.com/openclaw/gogcli) (≥ v0.44.0). `list` is the bare
default; a later phase adds vault synchronization.

Typical order: `bob gcal login` once, `bob gcal doctor` to confirm the
checklist, `bob gcal calendars` to copy calendar ids into config. Tests never
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
```

Running `bob gcal` with no subcommand runs `bob gcal list`. The list is
read-only. `sync` is not in this release.

## How it works

Bob talks to Google only through `gog`. Common flags go before the subcommand
and never include tokens:

`--no-input --json --color never --readonly --client bob-gcal --account <email>`

The isolated `--client bob-gcal` token bucket means removing or re-consenting
Bob's Calendar grant never touches other gog tokens (Gmail included). Login's
interactive `gog auth add` omits `--no-input --json` and inherits stdio so the
browser (or `-m` paste-URL) flow can run.

`id: primary` in config matches a calendar whose `primary` flag is true or
whose id is the literal `primary`.

## Setup

1. Install [gog](https://github.com/openclaw/gogcli) ≥ v0.44.0 and put it on
   `PATH`.
2. Add a `gcal:` block (see [Configuration](#configuration)). A missing block
   uses defaults.
3. Enable the Calendar API and download a Desktop-app OAuth client. The
   [connect Google Calendar](guides/connect-google-calendar.md) guide walks the
   Cloud Console checklist, including publishing the consent screen.
4. `bob gcal login -c PATH` (or `bob gcal login` to reuse a stored gog client).
5. `bob gcal doctor` until every check is `ok` or an explained skip.
6. `bob gcal calendars` to copy ids into `gcal.calendars`.

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
- `account` must contain `@` when set.
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

`bob gcal login` is one-time read-only Calendar consent.

1. **Client.** Use a stored `bob-gcal` client when one exists. Otherwise
   `-c FILE` runs `gog auth credentials set FILE --client bob-gcal`.
   Otherwise, on a TTY, offer to reuse another stored gog client
   (`Reuse gog client 'default' for Calendar? [Y/n]`). Otherwise print a
   four-step checklist and exit 2.
2. **Account.** `-a`, else `gcal.account`, else the sole gog account (listed
   without `--client` so other clients count), else a TTY prompt, else exit 2.
3. **Sign in.** `gog auth add <email> --client bob-gcal --services calendar --readonly`
   with inherited stdio, adding `--manual` under `-m`. When `SSH_CONNECTION` is
   set without `-m`, print a one-line hint to use `-m`, then continue. Login is
   idempotent: a valid grant reports `already connected` and skips sign-in.
4. **Check.** List calendars and the configured sync set. An
   `accessNotConfigured` error prints the Console link that enables the API:
   `https://console.cloud.google.com/apis/library/calendar-json.googleapis.com`.

Human output:

```text
Google Calendar · one-time setup · read-only

1/3  OAuth client   ✓ bob-gcal (Desktop app)
2/3  Sign in        ✓ already connected · you@example.com
3/3  Check          ✓ 8 calendars (syncing 1: primary)

Token stored by gog. Remove it with: bob gcal logout
Next: bob gcal calendars    list calendars for config
```

`invalid_grant` explains the 7-day expiry for consent screens left in Testing:
publish the consent screen, then run `bob gcal login`.

## Logout

`bob gcal logout -y` removes the isolated `bob-gcal` refresh token only.
Gmail and other gog clients stay. Then revoke Bob's Calendar access at
`https://myaccount.google.com/permissions`. Without `-y`/`--force`, logout
asks for confirmation on a TTY and exits 2 on a pipe.

## Doctor

`bob gcal doctor` walks gog, client, account and scope, a live calendars call
(the token refresh), configured calendars, default-root validity and its type
suggestion, today's daily note, journal git rules, and the writer host.

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

The table shows local time, title, RSVP, selected root and source, and whether
sync would create a task or context row. `-a` also shows omitted events with the
reason. Counts include omitted events even when their rows are hidden.

JSON has `schema_version`, `day`, `calendars`, `events`, and `counts` fields.
Each event reports `time`, `title`, `answer`, `inclusion`, `reason`, and
`root.value`, `root.source`, and `root.warnings`. Descriptions and attendee
details are omitted. A root may be null when no candidate is valid; warnings
explain each rejected candidate and its fix.

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
one: a title-ending `@route` or description line containing only `@route`,
`gcal.colors` by event colour, the calendar's `root`, then `gcal.root` (default
`meet`). A root must resolve relative to the daily note, be an area at any depth
or a root project, and must not be `@unfiled`.

Invalid candidates fall through to the next source with a warning that includes
the R11 reason and fix. If a title route resolves, Bob removes the final token
from the displayed title. An unknown route stays in the title and is reported.
If the default root also fails, list shows no root and `doctor` reports the
invalid default. Root sources are reported as `@body`, `colour tomato`,
`calendar Health`, or `default`. The list has no vault-state column; sync adds
it later.

## Exit status

- `0` — success, including "already connected" and a doctor run with only
  warnings.
- `1` — runtime failure: gog, network, timeout, token, or a doctor check
  that failed.
- `2` — usage or setup error: clap usage errors, bad `gcal:` config, missing
  gog, gog older than v0.44.0, no client, or not logged in.

Errors print `bob gcal <cmd>: <message>` to stderr (a red `error` prefix on a
TTY) with an optional dim `  hint: …` line. In JSON mode, failures print
`{"schema_version":1,"ok":false,"error":{"kind":…,"message":…,"hint":…}}` to
stdout.

## JSON output

Every JSON document carries `schema_version: 1`. JSON never includes tokens,
descriptions, or attendees.

`login -f json`: `ok`, `already_connected`, `account`, `client`, `calendars`,
`syncing`, `syncing_ids`.

`logout -f json`: `ok`, `account`, `client`, `revoke_url`.

`doctor -f json`: `ok`, `host`, `checks` (`id`, `status`, `summary`, `fix`).

`calendars -f json`: `ok`, `calendars` (`id`, `name`, `access_role`, `color`,
`primary`, `synced`, `root`).

`list -f json`: `day`, configured `calendars`, classified `events`, and
`counts`. `-a` adds omitted event rows. Each event contains no provider
description or attendee list.

## Security

Tokens never appear in argv or logs. The `bob-gcal` client is a separate
bucket from Gmail. Tests use `you@example.com` and never a live cookie.

## Environment

| Variable | Effect |
| --- | --- |
| `BOB_DIR` | Vault root; defaults to `~/bob`. |
| `BOB_CONFIG_FILE` | Config file holding the `gcal:` section. |
| `BOB_GCAL_GOG` | Executable replacing `gog` on `PATH`. Tests point this at a fake. |
| `BOB_GCAL_ASSUME_TTY` | `1` or `true` treats stdin as a TTY for login reuse and logout confirm (test hook). |
| `BOB_NOW` | Local datetime override for today and relative list-day selection. |
| `SSH_CONNECTION` | Login prints a `-m` hint when this is set without `-m`. |

## Known limits

- `sync` is not in this release.
- Google forbids Calendar scopes on the device-code flow; use `-m` over SSH.
- Consent screens left in Testing expire in 7 days; publish them.

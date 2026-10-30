# Bob Gcal

`bob gcal` projects Google Calendar events onto today's daily note as Event
Tasks. This page covers the one-time setup: a read-only login through
[gog](https://github.com/openclaw/gogcli) (≥ v0.44.0), `doctor`, `calendars`,
and `logout`. `list` and `sync` arrive in a later release; until then a bare
`bob gcal` prints help.

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
bob gcal login [-a|--account EMAIL] [-c|--credentials FILE] [-f|--format human|json] [-m|--manual]
bob gcal logout [-a|--account EMAIL] [-f|--format human|json] [-y|--force]
```

Running `bob gcal` with no subcommand prints this help. `list` and `sync` are
not in this release.

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
- `colors` and `tentative` are stored for later phases.
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
| `BOB_NOW` | Pins today's daily note for doctor. |
| `SSH_CONNECTION` | Login prints a `-m` hint when this is set without `-m`. |

## Known limits

- `list` and `sync` are not in this release.
- Colour name/id validation and classified today-counts belong to `gcal list`.
- Google forbids Calendar scopes on the device-code flow; use `-m` over SSH.
- Consent screens left in Testing expire in 7 days; publish them.

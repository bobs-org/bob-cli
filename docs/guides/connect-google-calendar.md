---
title: "Connect Google Calendar"
description: "Enable the Calendar API, store a Desktop-app client, sign in read-only through gog, and confirm the setup with doctor."
tags:
  - CLI
---

# Connect Google Calendar

This guide stores a read-only Google Calendar grant for Bob. Bob talks to
Google only through [gog](https://github.com/openclaw/gogcli) (≥ v0.44.0)
under an isolated `--client bob-gcal` token bucket, so removing Bob's grant
never touches Gmail. Tests and this page use `you@example.com`; do not paste
a live client secret into a log or ticket.

This page gets you through sign-in, doctor, calendar selection, and a first
sync.

## Before you start

Install the CLI, install gog ≥ v0.44.0 on `PATH`, and point Bob at your
vault. Add a `gcal:` block to `~/.config/bob/config.yml` (or
`$BOB_CONFIG_FILE`):

```yaml
gcal:
  account: you@example.com
  root: meet
  calendars:
    - id: primary
  timeout_secs: 60
```

`account` is optional when the isolated `bob-gcal` bucket holds exactly one
Google account, or when gog as a whole holds exactly one. A missing `gcal:`
block uses those defaults.

## 1. Let Bob find or create a Desktop-app client

Start with:

```bash
bob gcal login
```

Bob first checks for a saved `bob-gcal` client and a reusable gog client. It
also checks `~/Downloads` for the newest valid `client_secret*.json`. If it
does not find a client, follow the prompts to set one up:

In Google Cloud Console:

1. Enable the Calendar API at
   [calendar-json.googleapis.com](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com).
2. Set the OAuth consent screen to **External** and **In production**.
   Unverified is fine for personal use. Leave the screen in Testing and
   tokens expire in 7 days (`invalid_grant`); publish, then run
   `bob gcal login` again.
3. Create an OAuth client of type **Desktop app** and download the JSON.
4. Press Enter at Bob's prompt to search Downloads again, or paste the file
   path. You can also start with `bob gcal login -c PATH` to select a file
   directly.

Bob copies a reusable gog client automatically when its secret is available
in a file. If gog keeps the secret in the system keyring, Bob explains that
and asks for a downloaded client instead.

## 2. Sign in

```bash
bob gcal login
bob gcal doctor -b /path/to/vault
```

Bob signs in through gog's remote two-step flow and requests only read-only
Calendar access (`calendar.readonly`). It opens your browser on a desktop
session (`DISPLAY` or `WAYLAND_DISPLAY` on Linux; always on macOS) and uses
the paste-URL flow over SSH or on a headless host. On SSH, open the link in
any browser, allow access, then copy the full address from the expected
`127.0.0.1:<port>` page and paste it into Bob. Use `-m` to choose that flow
manually, or `BOB_GCAL_BROWSER` to select a browser executable.

Over SSH, reusing gog's Gmail client looks like this:

```text
$ bob gcal login -a you@example.com
Google Calendar · read-only · one-time setup

1/3  Google client  ✓ reusing gog's "default" client · project 123456789012
                    Bob keeps its own read-only sign-in; Gmail stays untouched.
2/3  Sign in        you@example.com · you're on SSH, so sign in from any browser:

https://accounts.google.com/o/oauth2/auth?…

                    1. Open the link, choose you@example.com, and allow
                       "See and download any calendar you can access".
                    2. "Google hasn't verified this app"? It's your own client:
                       Advanced → Go to … (unsafe).
                    3. You land on a 127.0.0.1 page that can't load. That's expected:
                       copy that page's whole address.

                    Paste the address (? for help) ›
                    ✓ signed in · read-only Calendar access granted
3/3  Check          ✓ 8 calendars · syncing 1: primary

✓ Connected. Bob can read, never change, you@example.com's calendars.

  Next  bob gcal sync -d     preview today's Events
        bob gcal calendars   choose which calendars to sync
  Undo  bob gcal logout      removes only Bob's Calendar grant
```

On a fresh desktop with no client stored yet, step 1 prints the Cloud Console
checklist, then the browser opens:

```text
$ bob gcal login
Google Calendar · read-only · one-time setup

1/3  Google client  Bob needs an OAuth client from your own Google Cloud project.
                    One time, about 3 minutes, in your browser:
                    …
2/3  Sign in        you@example.com · opening your browser…
                    Allow "See and download any calendar you can access".
                    Browser didn't open? Use this link:

https://accounts.google.com/o/oauth2/auth?…
```

A valid grant is idempotent: login still prints all three steps, reports
`already connected`, and rechecks calendars:

```text
1/3  Google client  ✓ bob-gcal · project 123456789012
2/3  Sign in        ✓ already connected · you@example.com
3/3  Check          ✓ 8 calendars · syncing 1: primary
```

If a saved grant expired or was revoked, Bob guides you through sign-in again.
If the Calendar API is disabled, Bob gives you the project-specific Cloud
Console link and lets you retry after enabling it. Without a TTY, that error
keeps the grant and tells you to enable the API, then rerun `bob gcal login`.

Confirm every doctor check is `ok` or an explained skip. A `no_client`
setup error means no usable OAuth client was found; run login in a terminal
and follow its checklist. Exit 1 on doctor is a failed check; the `fix:` line
names the next command.

## 3. Copy calendar ids into config

```bash
bob gcal calendars
```

The table shows id, name, access role, colour, whether Bob will sync it,
and the mapped root. Put those ids under `gcal.calendars`. `primary` matches
the account's primary calendar.

## 4. Preview, then run the first sync

```bash
bob gcal sync -d
bob gcal sync
```

The preview prints the exact unified diff of the plan and writes nothing.
The write adds a `## Events` section above `## Pomodoros`, one row per
accepted event in time order, plus a `() — MEET` placeholder carrying a link
per unended meeting. Meetings that already ended arrive as `[ ]` rows with no
link; later meetings arrive `[*]` and linked. Run `bob gcal sync` again to
confirm the second run is byte-identical and appends nothing, then
`bob gcal doctor` to see the `last sync` check go green.

## 5. Remove the grant

```bash
bob gcal logout -y
```

That removes only the `bob-gcal` token. Then revoke Bob's Calendar access at
[myaccount.google.com/permissions](https://myaccount.google.com/permissions).
Without `-y`, logout asks for confirmation on a TTY.

## If it goes wrong

| Symptom | What to do |
| --- | --- |
| `gog not found` | Install gog ≥ v0.44.0 and put it on `PATH`, or set `BOB_GCAL_GOG`. |
| `gog … is older than v0.44.0` | Upgrade gog. |
| `no_client`, exit 2 | Run login in a terminal and follow its API, consent screen, and Desktop-app client checklist. |
| Client JSON rejected | Use a Google OAuth client of type Desktop app; service-account, web, and gcloud credential files do not work. |
| gog client secret is in keyring | Download a Desktop-app JSON and pass its path to `bob gcal login -c PATH`. |
| `needs_tty` | Run `bob gcal login` in a terminal (paste and client setup cannot run on a pipe). |
| `cancelled` | Deliberate stop (`q`, Ctrl-D, or paste `access_denied` then `q`). Rerun `bob gcal login`. |
| `consent_denied` | Google refused access. Rerun `bob gcal login` and allow Calendar access. |
| `stale_link` | The pasted address or exchanged code is out of date. Rerun `bob gcal login` for a fresh link. |
| `accessNotConfigured` / `api_disabled` | Enable the Calendar API at the Console link login prints. Sign-in is kept; rerun `bob gcal login`. |
| `invalid_grant` / 7-day hint | Publish the consent screen, then `bob gcal login`. |
| `insufficient_scope` | `bob gcal login` to grant `calendar.readonly`. |
| `account_ambiguous` | Set `gcal.account` to the `bob-gcal` address Bob should use. |
| `wrong_account` | Choose the requested account on Google's picker, or pass `-a`. |
| Access blocked: app not verified | Publish the consent screen, or add yourself as a test user, on the Audience page. |
| `Error 400: redirect_uri_mismatch` | The client is not a Desktop app. Rerun `bob gcal login -c` with Desktop-app JSON. |
| "This site can't be reached" after Allow | Expected on paste. Copy that `127.0.0.1` page's whole address. |
| Browser didn't open | Use the printed link, or set `BOB_GCAL_BROWSER`. |
| Doctor `default root` fails | Create `meet.md` as `type: area`, or set `gcal.root` to an existing area. |
| Doctor `journal` fails | Add `!/.bob/gcal/*.jsonl` to `.gitignore` and `.bob/gcal/*.jsonl merge=union` to `.gitattributes`. |
| Doctor `writer` fails | Run setup on the pinned host, or set `gcal.writer` to this host's vault-sync slug. |

Exit 2 is setup (clap, config, missing gog, no client, not logged in). Exit 1
is runtime (network, timeout, token, or a doctor failure).

## Related

- [Bob Gcal](../gcal.md)
- [bob gcal](../reference/cli/gcal.md)
- [Configuration](../reference/configuration.md#gcal)

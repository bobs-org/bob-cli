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

`list` and `sync` arrive in a later release. This page gets you to
`bob gcal doctor` all-clear and a calendar id list for config.

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

`account` is optional when gog already holds exactly one Google account.
A missing `gcal:` block uses those defaults.

## 1. Create a Desktop-app client

In Google Cloud Console:

1. Enable the Calendar API at
   [calendar-json.googleapis.com](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com).
2. Set the OAuth consent screen to **External** and **In production**.
   Unverified is fine for personal use. Leave the screen in Testing and
   tokens expire in 7 days (`invalid_grant`); publish, then run
   `bob gcal login` again.
3. Create an OAuth client of type **Desktop app** and download the JSON.
4. Run `bob gcal login -c PATH` with that file.

If gog already stores another client, `bob gcal login` on a TTY offers
`Reuse gog client 'default' for Calendar? [Y/n]` instead of `-c`.

## 2. Sign in

```bash
bob gcal login -c ~/Downloads/client_secret.json
bob gcal doctor -b /path/to/vault
```

`login` opens a browser for read-only Calendar consent
(`calendar.readonly`). Over SSH, the browser cannot open: rerun with
`bob gcal login -m` and paste the redirect URL. When `SSH_CONNECTION` is
set without `-m`, login prints that hint and continues.

A valid grant is idempotent: login reports `already connected` and only
re-lists calendars.

Confirm every doctor check is `ok` or an explained skip. Exit 2 with the
four-step checklist means no `bob-gcal` client is stored. Exit 1 on doctor
is a failed check; the `fix:` line names the next command.

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
| Four-step checklist, exit 2 | Enable the API, publish the consent screen, download a Desktop-app client, `bob gcal login -c PATH`. |
| `accessNotConfigured` | Enable the Calendar API at the Console link doctor/login print. |
| `invalid_grant` / 7-day hint | Publish the consent screen, then `bob gcal login`. |
| `insufficient_scope` | `bob gcal login` to grant `calendar.readonly`. |
| Doctor `default root` fails | Create `meet.md` as `type: area`, or set `gcal.root` to an existing area. |
| Doctor `journal` fails | Add `!/.bob/gcal/*.jsonl` to `.gitignore` and `.bob/gcal/*.jsonl merge=union` to `.gitattributes`. |
| Doctor `writer` fails | Run setup on the pinned host, or set `gcal.writer` to this host's vault-sync slug. |

Exit 2 is setup (clap, config, missing gog, no client, not logged in). Exit 1
is runtime (network, timeout, token, or a doctor failure).

## Related

- [Bob Gcal](../gcal.md)
- [bob gcal](../reference/cli/gcal.md)
- [Configuration](../reference/configuration.md#gcal)

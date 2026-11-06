---
title: "Connect Google Mail"
description: "Enable the Gmail API, store a Desktop-app client, sign in through gog, and confirm the setup with doctor."
tags:
  - CLI
---

# Connect Google Mail

This guide stores a Gmail grant for Bob under an isolated `--client bob-gmail`
token bucket, so removing Bob's grant never touches Calendar. Tests and this
page use `you@example.com`; do not paste a live client secret into a log or
ticket.

This page gets you through sign-in and doctor. `bob gmail new` and
`bob gmail reply` create local compose notes without signing in. Save and list
selected threads with the commands in [Bob Gmail](../gmail.md), then link saved
threads to tasks with their labelled wikilinks. `bob gmail draft NOTE` requires
the optional compose grant from `bob gmail login -s compose`; it creates or
updates a Gmail draft and shows its stored preview without sending.
`bob gmail send` transmits only that stored draft after a confirmation bound
to its preview; see [Send email from Obsidian](send-email-from-obsidian.md).
Bare `bob gmail` lists saved threads locally.

## Before you start

Install the CLI, install gog ≥ v0.44.0 on `PATH`, and point Bob at your
vault. Add a `gmail:` block to `~/.config/bob/config.yml` (or
`$BOB_CONFIG_FILE`):

```yaml
gmail:
  account: you@example.com
  folder: mail
  timeout_secs: 60
```

`account` is optional when the isolated `bob-gmail` bucket holds exactly one
Google account, or when gog as a whole holds exactly one. A missing `gmail:`
block uses defaults. Keep the compose scope off agent hosts; grant it only on
the Mac.

## 1. Let Bob find or create a Desktop-app client

Start with:

```bash
bob gmail login
```

Bob first checks for a saved `bob-gmail` client and a reusable gog client. It
also checks `~/Downloads` for the newest valid `client_secret*.json`. If it
does not find a client, follow the prompts to set one up:

In Google Cloud Console:

1. Enable the Gmail API at
   [gmail.googleapis.com](https://console.cloud.google.com/apis/library/gmail.googleapis.com).
2. Set the OAuth consent screen to **External** and **In production**.
   Unverified is fine for personal use. Leave the screen in Testing and
   tokens expire in 7 days (`invalid_grant`); publish, then run
   `bob gmail login` again.
3. Create an OAuth client of type **Desktop app** and download the JSON.
4. Press Enter at Bob's prompt to search Downloads again, or paste the file
   path. You can also start with `bob gmail login -c PATH` to select a file
   directly.

Bob copies a reusable gog client automatically when its secret is available
in a file. If gog keeps the secret in the system keyring, Bob explains that
and asks for a downloaded client instead.

## 2. Sign in

```bash
bob gmail login
bob gmail doctor -b /path/to/vault
```

Bob signs in through gog's remote two-step flow and requests `--gmail-scope
readonly`. It opens your browser on a desktop session and uses the paste-URL
flow over SSH or on a headless host. On SSH, open the link in any browser,
allow access, then copy the full address from the expected
`127.0.0.1:<port>` page and paste it into Bob. Use `-m` to choose that flow
manually, or `BOB_GMAIL_BROWSER` to select a browser executable.

Read-only hosts stop here. On the Mac only, also grant compose:

```bash
bob gmail login -s compose
```

That also requests `https://www.googleapis.com/auth/gmail.compose` through
`--extra-scopes`. The docs say plainly that this scope also permits sending;
the gate is the preview/confirm step plus keeping that grant off agent hosts.

A valid grant is idempotent: login still prints all three steps, reports
`already connected`, rechecks one Gmail search page, and reports the granted
scopes (`readonly` or `readonly+compose`).

If a saved grant expired or was revoked, Bob guides you through sign-in again.
If the Gmail API is disabled, Bob gives you the project-specific Cloud
Console link and lets you retry after enabling it. Without a TTY, that error
keeps the grant and tells you to enable the API, then rerun `bob gmail login`.

Confirm every doctor check is `ok` or an explained skip.

## 3. Remove the grant

```bash
bob gmail logout -y
```

That removes only the `bob-gmail` token. Then revoke Bob's Gmail access at
[myaccount.google.com/permissions](https://myaccount.google.com/permissions).
Without `-y`, logout asks for confirmation on a TTY.

## If it goes wrong

| Symptom | What to do |
| --- | --- |
| `gog not found` | Install gog ≥ v0.44.0 and put it on `PATH`, or set `BOB_GMAIL_GOG`. |
| `gog … is older than v0.44.0` | Upgrade gog. |
| `no_client`, exit 2 | Run login in a terminal and follow its API, consent screen, and Desktop-app client checklist. |
| Client JSON rejected | Use a Google OAuth client of type Desktop app. |
| gog client secret is in keyring | Download a Desktop-app JSON and pass its path to `bob gmail login -c PATH`. |
| `needs_tty` | Run `bob gmail login` in a terminal. |
| `cancelled` | Deliberate stop (`q`, Ctrl-D, or paste `access_denied` then `q`). Rerun `bob gmail login`. |
| `stale_link` | The pasted address is out of date. Rerun `bob gmail login` for a fresh link. |
| `accessNotConfigured` / `api_disabled` | Enable the Gmail API at the Console link login prints. Sign-in is kept; rerun `bob gmail login`. |
| `invalid_grant` / 7-day hint | Publish the consent screen, then `bob gmail login`. A Google password change revokes Gmail tokens; a token unused for 6 months expires. |
| `insufficient_scope` | `bob gmail login` to grant Gmail scopes. |
| `account_ambiguous` | Set `gmail.account` to the `bob-gmail` address Bob should use. |
| `wrong_account` | Choose the requested account on Google's picker, or pass `-a`. |
| `Error 400: redirect_uri_mismatch` | The client is not a Desktop app. Rerun `bob gmail login -c` with Desktop-app JSON. |
| Browser didn't open | Use the printed link, or set `BOB_GMAIL_BROWSER`. |
| Doctor `journal` fails | Add `!/.bob/gmail/*.jsonl` to `.gitignore` and `.bob/gmail/*.jsonl merge=union` to `.gitattributes`. |
| Doctor `writer` fails | Run writes on the pinned host, or set `gmail.writer` to this host's slug. |

Exit 2 is setup (clap, config, missing gog, no client, not logged in). Exit 1
is runtime (network, timeout, token, or a doctor failure).

## Try it

On your own vault (never in tests):

```bash
bob gmail login
bob gmail doctor
bob gmail
bob gmail logout -y
```

`bob gmail` lists your saved threads; a new vault has none yet. `doctor`
should be green before you save one with `bob gmail save 'GMAIL_URL'`.

## Related

- [Bob Gmail](../gmail.md)

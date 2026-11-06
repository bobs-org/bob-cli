# Bob Gmail

`bob gmail` saves Gmail threads you choose as inert, append-only mail notes in
the vault, linked from tasks with a readable label. This page covers one-time
setup through [gog](https://github.com/openclaw/gogcli) (≥ v0.44.0) and the
shell: the `gmail` noun group, config, login, logout, and doctor. Saving,
linking, search, and sending land in later phases; bare `bob gmail` currently
lists no saved threads yet and never touches the network.

Typical order: `bob gmail login` once, `bob gmail doctor` until every check is
`ok` or an explained skip. Tests never touch Google: set `BOB_GMAIL_GOG` to a
fake executable.

## Contents

- [Commands](#commands)
- [How it works](#how-it-works)
- [Body rendering](#body-rendering)
- [Setup](#setup)
- [Configuration](#configuration)
- [Login](#login)
- [Logout](#logout)
- [Doctor](#doctor)
- [Where it runs](#where-it-runs)
- [Exit status](#exit-status)
- [JSON output](#json-output)
- [Security](#security)
- [Environment](#environment)
- [Known limits](#known-limits)

## Commands

```bash
bob gmail [-b|--bob-dir DIR] [-f|--format table|json]
bob gmail doctor [-b|--bob-dir DIR] [-f|--format human|json]
bob gmail login [-a|--account EMAIL] [-c|--credentials FILE] [-f|--format human|json] [-m|--manual] [-s|--scope readonly|compose]
bob gmail logout [-a|--account EMAIL] [-f|--format human|json] [-y|--force]
```

Running `bob gmail` with no subcommand lists saved threads. Until `save`
lands, it prints a friendly "no saved threads yet" view and makes no Gmail
calls. `doctor` is read-only; `login` and `logout` are excluded from the
command catalog (they need a terminal and handle credentials).

## How it works

Bob talks to Google only through `gog`. Read flags go before the subcommand
and never include tokens:

`--no-input --json --color never --readonly --client bob-gmail --account <email> --gmail-no-send --enable-commands gmail.thread.get,gmail.get,gmail.messages,gmail.search,gmail.url,gmail.attachment`

The isolated `--client bob-gmail` token bucket means removing or re-consenting
Bob's Gmail grant never touches other gog tokens (Calendar included). Login
runs gog's remote two-step sign-in with captured `--no-input --json` input and
output. Client JSON goes to gog on stdin. Step 2 passes the single-use,
PKCE-bound redirect URL as `--auth-url`; the code is useless without gog's
stored verifier, and gog redeems it immediately. Bob owns the browser and
paste interaction, so gog never takes over the terminal.

Reads always pass `--readonly --gmail-no-send` plus `--enable-commands`
restricted to the Gmail read commands, and never pass `--sanitize-content` or
`--wrap-untrusted`. The bodies live only in mail notes, never in argv, logs,
journals, or JSON errors.

## Body rendering

When `save` lands, each message's MIME tree renders to inert Markdown through
a pure boundary with no network, filesystem, or clock access. The same rules
already decide what the saved notes will contain:

- HTML is preferred when it carries links, tables, or referenced images, or
  when it says something the plain version does not. A bare tag wrapper
  around identical text falls back to plain, then to the snippet. Both
  alternatives are never concatenated.
- Quoted history (`>` runs, `On ... wrote:` tails, Outlook forwards) and the
  `-- ` signature fold into separate sections, so the note can hide them
  behind one folded callout. Inline replies stay in the main text.
- Bulk and list mail (a `List-Unsubscribe` or `List-Id` header, a `bulk`
  precedence, or a Gmail promotion/update/forum category) renders in bulletin
  mode: flattened text with safe links, no images, no import candidates.
- Remote images never load and never become image syntax; a safe link labelled
  `🖼` plus the alt text or host is the most a remote image becomes. Tracking
  pixels are dropped. A `cid:` image resolves only against that message's own
  MIME resources and stays a typed descriptor until `save` copies a validated
  local file; nothing sender-controlled becomes a local embed on its own.
- Body parts whose bytes were not fetched yet keep a fetch marker instead of
  masquerading as empty bodies, and re-rendering with the fetched bytes fills
  them in. `DRAFT` messages are never imported.
- Every sender word is neutralized at render time: wikilinks, tags, Dataview
  fields, checkboxes, Templater openers, and executable code fences cannot
  survive in a body, a header, a snippet, or a filename shown in the note.
  Raw filenames and IDs stay untrusted path inputs for the save phase.

## Setup

1. Install [gog](https://github.com/openclaw/gogcli) ≥ v0.44.0 and put it on
   `PATH`.
2. Add a `gmail:` block (see [Configuration](#configuration)). A missing block
   uses defaults.
3. Run `bob gmail login`. It reuses a stored Bob client or a readable gog
   client, checks `~/Downloads` for a Desktop-app client, and guides you
   through Google Cloud setup if needed. You can provide a client directly
   with `-c PATH`. The [connect Google mail](guides/connect-google-mail.md)
   guide explains each Cloud Console screen.
4. `bob gmail doctor` until every check is `ok` or an explained skip.

## Configuration

`bob gmail` reads `~/.config/bob/config.yml` (override with `BOB_CONFIG_FILE`):

```yaml
gmail:
  account: you@example.com # optional when gog holds exactly one account
  body: full # full | headers (default full)
  folder: mail # vault folder for mail notes
  link_label: "✉️ {subject} · {who}" # must contain {subject}
  never_save: # threads that never save
    from: [] # addresses or @domains, case-insensitive
    labels: [] # Gmail label names
  pull:
    inbox: gmail_inbox.md # task file for label pulls
    label: bob # unset or empty disables pull
  signature: gmail # gmail | none
  timeout_secs: 60
  writer: mac-host # optional; the one host label allowed to write
```

- A missing or null `gmail:` block means defaults. Unknown keys are ignored.
- A wrong-typed value is a setup error (exit 2) that names the key, for
  example `gmail.timeout_secs`, and never breaks other config loaders.
- `account` must contain `@` when set. Commands resolve it in this order:

  | Source | When it wins |
  | --- | --- |
  | `bob gmail login -a EMAIL` | Explicit override (login only) |
  | `gmail.account` | Configured |
  | Sole `bob-gmail` account | The isolated bucket holds exactly one unique email |
  | Sole gog account | No Bob grant, and `gog auth list` holds exactly one unique email |
  | TTY picker | Login on a terminal when more than one email remains |
  | `account_ambiguous` | Other commands when more than one email remains |

  Unique emails are compared case-insensitively.
- `body` is `full` (default) or `headers`. Later `save` honors it; headers
  mode keeps only the snippet plus a "(body not saved)" marker.
- `folder` is the vault folder for mail notes. Default `mail`.
- `link_label` templates the task link. Default `✉️ {subject} · {who}` under
  the `subject_who` decision. It must contain `{subject}`.
- `never_save.from` lists addresses or `@domains`; `never_save.labels`
  lists Gmail label names. Later `save` refuses them with a reason.
- `pull.label` unset or empty disables pull. Default `bob`; inbox default
  `gmail_inbox.md`.
- `signature` is `gmail` (default) or `none`. Later `compose` honors it.
- `timeout_secs` must be an integer greater than 0. Default 60.
- The host label is the same slug vault-sync uses in commit subjects.
  `doctor` prints it. Unset `writer` means this host may write; a mismatch
  fails the writer check.

## Login

`bob gmail login` owns a three-step Gmail sign-in. Progress and prompts go to
stderr; with `-f json`, stdout contains only the report and stderr shows only
the interactive pieces. Human mode leaves stdout empty. A healthy rerun is
idempotent: it still prints steps 1–3, `already connected`, the summary, and
the writer note.

1. **Google client.** `-c FILE` loads and validates a Desktop-app client,
   replacing Bob's saved client. Otherwise Bob uses its saved client, copies a
   readable gog client whose secret is not keyring-only, then checks the newest
   valid `~/Downloads/client_secret*.json`. On a TTY, if none is available, Bob
   explains how to create one in Cloud Console and accepts a downloaded path or
   pasted JSON. Client secrets go to gog over stdin.
2. **Sign in.** `-a` overrides `gmail.account`; when neither selects an
   account, Bob uses the sole `bob-gmail` account, else the sole gog account, or
   asks on a TTY. A saved valid grant is reused. Otherwise Bob selects browser
   sign-in when `BOB_GMAIL_BROWSER` is set, or when a desktop session is
   available. SSH and headless sessions use the paste flow. `-m` always selects
   paste. Login requests `--gmail-scope readonly`. `-s compose` also requests
   `https://www.googleapis.com/auth/gmail.compose` through `--extra-scopes`
   (for the Mac only); the docs say plainly that this scope also permits
   sending. The gate is the preview/confirm step plus keeping that grant off
   agent hosts.
3. **Check.** Bob runs one Gmail search page with `max=1` to confirm the grant
   and reports the granted scopes (`readonly` or `readonly+compose`). If the
   Gmail API is off, a TTY can turn it on and retry in place; without a TTY, or
   after `q`, the grant is kept and the hint says to enable the printed project
   URL then rerun `bob gmail login`. An expired or revoked grant (`invalid_grant`)
   signs in again automatically.

## Logout

`bob gmail logout` removes only the isolated `bob-gmail` refresh token for the
account. Calendar, Keep, and other gog clients are left alone. It prints the
Google revoke URL. Pass `-y` to skip the confirmation; without a TTY, `-y` is
required.

## Doctor

`bob gmail doctor` checks gog, the stored `bob-gmail` client, the account, the
token (one `search` page), the granted scopes, the config, the writer pin, and
journal readability. Rows look like `bob gcal doctor`:

- `gog`: version and program path.
- `client`: stored `bob-gmail` OAuth client and its Cloud project.
- `account`: the resolved email and its Gmail grant.
- `token`: `refreshes`, or `invalid_grant` with a login hint. Testing-mode
  consent screens issue 7-day tokens; a Google password change revokes Gmail
  tokens; a token unused for 6 months expires.
- `scopes`: `readonly` or `readonly+compose`.
- `config`: folder, body mode, and pull label.
- `writer`: this host, or the pin mismatch.
- `journal`: `.bob/gmail/*.jsonl` tracked and union-merged.
- `journal_malformed`: malformed lines skipped with a count.

Exit 1 when any check fails. `-f json` prints `schema_version: 1`, `ok`, the
host, and the typed checks.

## Where it runs

Any logged-in host may read Gmail. Vault writes run only on `gmail.writer`
when that key is set; the docs recommend the Mac. Agent hosts stay read-only:
the compose scope is granted only on the Mac. `doctor` reports the writer pin
and the current host.

## Exit status

0 success, 1 runtime failure, 2 usage or setup error. Config errors name the
key (for example `gmail.account`).

## JSON output

Every `-f json` output carries `schema_version: 1` and `ok`. Errors are typed
`{kind, message, hint?}` on stdout; human errors go to stderr with a dim hint.

## Security

- Tokens never appear in argv, logs, journals, diagnostics, caches, or JSON
  errors. Client JSON travels to gog over stdin.
- Bodies, raw MIME, and attachment bytes never appear outside mail notes.
  Reads never pass `--sanitize-content` or `--wrap-untrusted`.
- `gmail.compose` also permits sending. Bob never sends without a Gmail draft
  plus a digest-bound confirmation, and keeps that grant off agent hosts.
- Tests use a fake gog and temp vaults. Committed fixtures are synthetic.

## Environment

- `BOB_DIR`: vault root; defaults to `~/bob`.
- `BOB_CONFIG_FILE`: config file; defaults to `~/.config/bob/config.yml`.
- `BOB_GMAIL_GOG`: gog executable replacing `gog` on PATH.
- `BOB_GMAIL_BROWSER`: browser for sign-in (otherwise automatic).
- `BOB_GMAIL_ASSUME_TTY`: `1`/`true` treats stdin as a TTY for prompts.
- `BOB_GMAIL_CONSENT_SECS`: consent window override (tests).

## Known limits

Saving, linking, search, peeking, composing, and pulling land in later phases.
Bare `bob gmail` lists no saved threads yet; `doctor` does not check decoder
health (that arrives with `save`).

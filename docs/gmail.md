# Bob Gmail

`bob gmail` saves Gmail threads you choose as inert, append-only mail notes in
the vault, swaps Gmail URLs in notes for readable links, and creates local
compose and reply notes. It also files labelled threads into area or project
notes (falling back to the Gmail inbox), watches those open tasks for new
outgoing and incoming messages, and imports selected attachments.
Sending goes through a previewed Gmail draft:
`draft` stores the draft and shows its exact preview, and `send` transmits it
only after a confirmation bound to that preview. This page covers one-time
setup through [gog](https://github.com/openclaw/gogcli) (≥ v0.44.0), config,
login/logout, doctor, saving, linking, listing, search, previews, label sync,
watch holds and lifts, attachment imports, compose notes, drafts, and confirmed
sends. `new` and `reply` write local notes without sending mail. Bare
`bob gmail` lists saved threads offline.

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
- [Save](#save)
- [Compose notes](#compose-notes)
- [Gmail drafts](#gmail-drafts)
- [Outgoing message rendering](#outgoing-message-rendering)
- [Writing and sending](#writing-and-sending)
- [List](#list)
- [Link](#link)
- [Search](#search)
- [Peek](#peek)
- [Sync](#sync)
- [Watch](#watch)
- [Attachments](#attachments)
- [Try it](#try-it)
- [Doctor](#doctor)
- [Where it runs](#where-it-runs)
- [Exit status](#exit-status)
- [JSON output](#json-output)
- [Security](#security)
- [Environment](#environment)
- [Known limits](#known-limits)

## Commands

```bash
bob gmail [list] [-a|--all] [-b|--bob-dir DIR] [-f|--format table|json] [-r|--remote]
bob gmail attach NOTE N [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json]
bob gmail doctor [-b|--bob-dir DIR] [-f|--format human|json]
bob gmail draft NOTE [-b|--bob-dir DIR] [-f|--format human|json]
bob gmail link NOTE... [-a|--all] [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-q|--quiet]
bob gmail login [-a|--account EMAIL] [-c|--credentials FILE] [-f|--format human|json] [-m|--manual] [-s|--scope readonly|compose|modify]
bob gmail logout [-a|--account EMAIL] [-f|--format human|json] [-y|--force]
bob gmail new [-b|--bob-dir DIR] [-B|--bcc LIST] [-c|--cc LIST] [-f|--format human|json] [-s|--subject TEXT] [-t|--to LIST]
bob gmail peek URL|ID [-f|--format human|json]
bob gmail reply NOTE [-a|--all] [-b|--bob-dir DIR] [-f|--format human|json]
bob gmail save URL|ID|NOTE... [-b|--bob-dir DIR] [-B|--body full|headers] [-d|--dry-run] [-f|--format human|json] [-P|--parent ROUTE] [-q|--quiet] [-R|--rerender]
bob gmail search [QUERY] [-f|--format table|json] [-l|--limit N]
bob gmail send NOTE [-b|--bob-dir DIR] [-c|--confirm DIGEST] [-d|--dry-run] [-f|--format human|json]
bob gmail sync [-b|--bob-dir DIR] [-c|--confirm DIGEST] [-d|--dry-run] [-f|--format human|json] [-l|--limit N] [-n|--no-archive] [-q|--quiet] [-r|--retry-timeout SECS]
```

Running `bob gmail` with no subcommand lists saved threads, exactly like the
explicit `bob gmail list`. Listing is offline and read-only by default; `-r`
fetches live message counts for the rows shown. `save` reads Gmail and writes
selected threads to the vault. `link` finds Gmail URLs in notes, saves each
confirmed thread, and replaces those URL tokens with labelled wikilinks.
`sync` reads the configured label and files saved threads into the area or
project named by a routing label, or into the Gmail inbox when none or an
invalid one is present. By default it leaves Gmail labels unchanged; the
archive-on-close path below is the only message-label mutation. Each run also refreshes
currently watched threads and applies a new counted turn to their open linking
tasks. `attach` reads one listed attachment and writes
its bytes into the vault. `new` and `reply` write local Markdown notes without
login or network access. `draft` stores a Gmail draft and shows its exact
preview without sending. `send` transmits only the stored draft after a
confirmation bound to that preview. `doctor`, `search`, and `peek` are
read-only. `login` and `logout` are excluded from the command catalog because
they need a terminal and handle credentials; `new` and `reply` are excluded
because the compose flow is owned by Bob Mail.

## How it works

Bob talks to Google only through `gog`. Read flags go before the subcommand
and never include tokens:

`--no-input --json --color never --readonly --client bob-gmail --account <email> --gmail-no-send --enable-commands gmail.thread.get,gmail.get,gmail.messages,gmail.search,gmail.url,gmail.labels.list`

Attachment bodies use a narrower Discovery read (`api call gmail v1
gmail.users.messages.attachments.get --params … --scope
https://www.googleapis.com/auth/gmail.readonly --no-cache`) with
`--enable-commands api.call,api.gmail.users.messages.attachments.get`.
The legacy `gmail attachment` command is never used: it writes a cache file.

IDs-only listing (sync discovery and `rfc822msgid:` resolution) uses
`api call gmail v1 gmail.users.threads.list --params … --scope
https://www.googleapis.com/auth/gmail.readonly --no-cache` with
`--enable-commands api.call,api.gmail.users.threads.list`. Hydrated
`gmail search` serves only the picker, login and token checks, and the
sent-message lookup.

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

Archive writes run without `--readonly` and fence the mutation to the exact
receipt/`INBOX` label pair: batch archive uses
`--enable-commands-exact gmail.batch.modify`, receipt creation uses
`--enable-commands-exact gmail.labels.create`. The fence never includes send,
delete, mark-read, or any other label.

## Body rendering

When a thread is saved, each message's MIME tree renders to inert Markdown through
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
  # gog: ~/bin/gog # optional absolute path or wrapper
  body: full # full | headers (default full)
  folder: mail # vault folder for mail notes
  link_label: "✉️ {subject} · {who}" # must contain {subject}
  never_save: # threads that never save
    from: [] # addresses or @domains, case-insensitive
    labels: [] # Gmail label names
  pull: # older spelling of sync.label and sync.inbox
    inbox: gmail_inbox.md # task file for label pulls
    label: bob # older spelling of sync.label; missing falls through to bob; "" disables
  signature: gmail # gmail | none
  sync:
    label: bob # enrollment label; falls back to pull.label, then bob; "" disables sync
    inbox: gmail_inbox.md # unrouted threads and route problems; falls back to pull.inbox
    route_prefixes: ["#", "+"] # quote them; absent or [] = no-prefix mode
    reply_wait_days: 3 # integer >= 1; calendar days after your send
    archive: off # off | on; opt in to one-shot archive after all linking tasks close
    archive_label: bob/closed # receipt label; defaults to <resolved sync label>/closed
    archive_max: 10 # positive integer; larger batches require -c DIGEST
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
- `body` is `full` (default) or `headers`. `save` honors it; headers
  mode keeps only the snippet plus a "(body not saved)" marker.
- `folder` is the vault folder for mail notes. Default `mail`.
- `link_label` templates the task link. Default `✉️ {subject} · {who}` under
  the `subject_who` decision. It must contain `{subject}`.
- `never_save.from` lists addresses or `@domains`; `never_save.labels`
  lists Gmail label names. `save` refuses matching threads with a reason.
- `pull.label` is the older spelling of `sync.label`. A missing value falls
  through to `bob`; an empty string disables sync. Inbox default
  `gmail_inbox.md`. See the [phone filing guide](guides/file-email-from-your-phone.md).
- `pull.inbox` must be a safe relative path inside the vault. If it is
  missing, Bob creates it with `parent: "[[inbox]]"`, `type: "[[area]]"`,
  `ready_cap: off`, and a `## Tasks` section.
- `pull.label` and `pull.inbox` are the older spelling of `sync.label` and
  `sync.inbox`. They keep working as fallbacks.
- `sync.label` is the enrollment label: applying it to a thread enrolls the
  thread once. An unset or null `sync.label` falls back to `pull.label`, then
  `bob`. An empty string disables sync in either spelling.
- `sync.inbox` receives unrouted threads and route problems. It falls back to
  `pull.inbox`, then `gmail_inbox.md`, and must be a safe relative path inside
  the vault.
- `sync.route_prefixes` turns on prefix mode. A user label named
  `<prefix>/<note>` for a listed prefix, such as `#/job` or
  `+/job_2026_apply_29`, is a routing label. It names one area or project note
  by stem or `project_name_aliases`, case-insensitively, the way
  `bob gmail save -P` does.
  - Quote each entry: a bare `#` starts a YAML comment. Entries must be
    non-empty and distinct, with no `/` and no whitespace.
  - A bare `#` parent label, unprefixed labels, and system labels are never
    routes.
  - Every routing label on a thread must resolve. One that doesn't is a route
    problem, even when another label resolves.
  - Several resolving labels route to their deepest note when they name one
    note or lie on one `parent:` chain (project → parent project → area).
    Otherwise the thread is a route problem.
  - Absent or `[]` is no-prefix mode: every user label except the enrollment
    label that names an area or project routes, and other labels are ignored.
  - Route problems are `unknown_note`, `not_area_or_project`,
    `closed_project`, `ambiguous_alias`, `nested_route` (a `<note>` that
    contains `/`), and `ambiguous_route`. Bob never creates an area or project
    note.
- `sync.reply_wait_days` must be an integer of at least 1. Default 3. It counts
  calendar days from your send to the awaiting-reply nudge date. Changing it
  never reschedules existing holds.
- `sync.archive` is `off` (default) or `on`. When enabled, the approved writer
  can archive enrolled Gmail messages after every linking task is positively
  closed. `sync.archive_label` names the receipt label and defaults to the
  resolved enrollment label plus `/closed`; Bob creates it on the first
  approved archive run. It cannot equal or alias the enrollment label.
  `sync.archive_max` is a positive limit (default 10); larger eligible sets
  pause as a whole until confirmed with the preview digest.
- `signature` is `gmail` (default) or `none`; remote Gmail drafts include the
  account's Gmail signature or omit it. The stored-draft preview shows the result.
- `timeout_secs` must be an integer greater than 0. Default 60.
- `gog`, when set, must be an absolute executable path; a leading `~/` is
  expanded. Bob resolves the executable in this order: `BOB_GMAIL_GOG`,
  `gmail.gog`, `gog` on `PATH`, `~/.local/bin/gog`, `/opt/homebrew/bin/gog`,
  then `/usr/local/bin/gog`.
- Every gog child receives `XDG_CONFIG_HOME` set to Bob's config root: the
  non-empty value from Bob's environment after `~/` expansion, or
  `$HOME/.config`. gog's store is `<root>/gogcli`. A macOS store left under
  `~/Library/Application Support/gogcli` is not read; rerun `bob gmail login`
  or move `gogcli` into the configured root.
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
   `https://www.googleapis.com/auth/gmail.compose`; Google notes that this
   permits sending, and Bob still requires its separate draft preview and
   confirmation. `-s modify` requests Gmail's broader modify scope on the
   approved writer. Google may allow more than Bob uses: Bob's archive path is
   fenced to adding the configured receipt label and removing `INBOX` from
   saved message IDs; it cannot permanently delete mail. Moving a thread back
   to Inbox is always explicit. A saved narrower grant does not satisfy a
   broader requested scope, so Bob signs in again for fresh consent.
3. **Check.** Bob runs one Gmail search page with `max=1` to confirm the grant
   and reports the granted scopes (`readonly`, `readonly+compose`, or the
   granted modify combination). With
   `-s compose`, a consent that leaves compose unticked fails with
   `compose_not_granted` (exit 1); the read-only grant is kept. The title,
   summary, and compose note follow the access requested and granted, so a
   read-only result never claims compose. If the
   Gmail API is off, a TTY can turn it on and retry in place; without a TTY, or
   after `q`, the grant is kept and the hint says to enable the printed project
   URL then rerun `bob gmail login`. An expired or revoked grant (`invalid_grant`)
   signs in again automatically.

## Logout

`bob gmail logout` removes only the isolated `bob-gmail` refresh token for the
account. Calendar, Keep, and other gog clients are left alone. It prints the
Google revoke URL. Pass `-y` to skip the confirmation; without a TTY, `-y` is
required.

## Save

`bob gmail save` accepts Gmail URLs, thread IDs, message IDs, RFC message IDs,
and paths to saved mail notes. It confirms the selected account and thread with
Gmail, applies `never_save`, prepares all inputs, then writes them together
under the vault maintenance lock. A failed input refuses the whole batch.
Repeated inputs for the same thread are fetched and written once; each input
still gets a result row.

- `-B full|headers` overrides `gmail.body` for this invocation. Full mode
  fetches body parts and referenced inline images into memory before the lock.
  Headers mode performs no body-part or image downloads.
- `-P ROUTE` resolves an area or project and sets `parent:` on newly created
  notes. Refresh keeps the note's existing parent and saved path.
- `-R` explicitly rebuilds managed message cards while preserving authored
  notes and historical cards no longer returned by Gmail.
- `-d` previews the target paths and structural changes. Message prose is
  redacted, and the command creates no lock, directories, notes, images,
  journals, or commits.
- `-f json` returns one body-free result per input with the confirmed identity,
  note path, labelled wikilink, status, message count, and number of new cards.
  A single input also has top-level `note` and `link` fields. `-q` suppresses
  human success output but does not suppress JSON or errors.

Mail notes are append-only snapshots. Refresh adds new message cards and
updates Bob-owned metadata while preserving authored text. Malformed markers,
reordered messages, duplicate identities, policy refusals, and path conflicts
stop before overwriting notes. A save commits only changed mail notes, imported
images, newly created `mail.md` / `mail.base` hubs, and an upgraded
`mail.base`. Save journals are appended after the commit (or after durable
writes in a non-Git vault).
`gmail.writer` blocks writes on other hosts; dry-run remains available there.

Every save or refresh also records whose turn the thread is in, in two
frontmatter fields right after `messages`:

- `turn: yours` when the newest counted message is theirs or a bounce, so the
  ball is in your court; `turn: theirs` when you wrote last.
- `turn_since`: the local time of that message.

Bob classifies each message from metadata only, first match wins:

| Message                                                                                         | Class          |
| ----------------------------------------------------------------------------------------------- | -------------- |
| Labelled `DRAFT`                                                                                | ignored        |
| `SENT`, or `From` is the account, and every To/Cc/Bcc address is the account                    | ignored (self) |
| `SENT`, or `From` is the account, with another recipient                                        | your move      |
| `From` is `mailer-daemon@` / `postmaster@`, or `multipart/report; report-type=delivery-status`  | bounce         |
| `Auto-Submitted` other than `no`, `X-Autoreply`, `X-Autorespond`, or `Precedence: auto_reply`   | automated      |
| Anything else with a sender address                                                             | their move     |

Messages are ordered by Gmail's `internalDate`, with thread order breaking
ties. Automated and ignored messages never decide the turn. Both fields are
omitted when nobody has made a move, when a message lacks `internalDate`, or
when the newest message has no `SENT` label and no sender address. A changed
turn counts as a refresh.

`mail.base` has two table views. Mail lists every saved thread. Waiting for
reply lists the notes with `turn == "theirs"`, oldest `turn_since` first, with
`title`, `who`, `turn_since`, and backlinks. A `mail.base` still byte-identical
to the earlier one-view template is upgraded in the same write that creates or
refreshes a mail note. Bob never touches a customized `mail.base`.

The dry-run preview lists note paths, changed owned metadata field names,
message-card identifiers, folding changes, and referenced or newly written
image paths. It never includes message prose, snippets, or before/after body
text. A byte-identical refresh reports `No structural changes.`

If the scoped Git commit fails, Bob reports that the note and images were
written but not committed and does not append success journals. Repeating the
save finishes the scoped commit, then records the Gmail save and newly
referenced image origins once. A later no-op save adds no journal rows. Image
imports are selected from the final merged note, so a full-body fetch does not
install images for existing header-only cards unless `-R` replaces those
cards.

Referenced CID images are copied under `img/mail/<thread id>/`, capped at
2 MiB, content-hash deduplicated, and embedded only as local trusted images.
Small images repeated across different messages are treated as signature
logos. Ordinary attachments remain listings. A one-line warning is added to
the vault root `AGENTS.md` once; vault-sync owns committing that warning.

## Compose notes

`bob gmail new` creates a standalone compose note under `gmail.folder`, using
the configured day timezone, the current date, the subject slug, and a numeric
suffix if the path is occupied. The note's path stays fixed after creation.
`--bob-dir` selects the vault. `--to`, `--cc`, and `--bcc` accept comma-separated
mailboxes; quote a display name that contains a comma. The `to`, `cc`, and `bcc`
frontmatter values are YAML sequences. An empty recipient list and an empty
subject are valid while authoring. Subject and recipient values reject control
characters and CR/LF injection.

The new note has `parent: "[[mail]]"`, `type: "[[mail]]"`, the `bob-mail` CSS
class, `status: draft`, recipient fields, `subject`, and an empty `attachments`
list. Its message body starts empty below the H1. The note is not indexed as a
saved Gmail thread until the send flow promotes it. `mail.md` and `mail.base`
are created only when missing. Writes honor `gmail.writer`, the vault
maintenance lock, symlink checks, staged preimage validation, and path-scoped
Git commits; unrelated dirty vault paths remain outside the commit.

`bob gmail reply NOTE` appends one `## Reply` section to a saved Gmail note;
`--all` appends `## Reply all`. `NOTE` may be a vault-relative path in
`gmail.folder` (with or without `.md`) or a unique note basename. The saved
note's managed message markers supply the exact thread, Gmail message, and RFC
message identities for later draft work. A second reply refuses without
changing the note. Existing notes, including `## Notes`, are preserved byte for
byte before the new section. Reply insertion is local and does not require a
Gmail login.

With `-f json`, both commands return schema 1 with `ok`, `status`, `note`, and
`cursor`. `status` is `created` for `new` and `reply_added` for `reply`. The
vault-relative note path is in `note`. Cursor `line` and `column` are both
zero-based; `column` counts UTF-16 code units to match the editor position
model. The cursor points to the start of the empty message body or Reply
section body.

```bash
bob gmail new -t 'Ada <ada@example.com>' -s "Lease renewal" -f json
bob gmail reply mail/2026/261105_lease_renewal.md --all -f json
```

## Gmail drafts

`bob gmail draft NOTE` creates a Gmail draft for a compose note or updates the
draft id already recorded on that note. It requires the configured
`gmail.writer` host and the isolated `bob-gmail` grant with the compose scope;
run `bob gmail login -s compose` to add that grant. Creating or updating a
draft never sends it.

Before calling Gmail, Bob renders the authored message region into plain-text
and HTML alternatives, validates and snapshots every declared attachment, and
persists a body-free create marker. The body and attachment bytes go to gog
through private temporary files. For replies, Bob checks the saved account,
thread, and selected non-draft message, then verifies the returned draft still
belongs to that thread. A Reply answers the selected message's sender; when
you sent that message, it answers that message's To recipients instead, as
Gmail does. Reply-all recipients are resolved by Gmail and are shown from the
stored draft.

Bob reads the stored draft back without using gog's download cache. It hydrates
both MIME alternatives and attachments in memory and refuses a preview if any
part is missing, ambiguous, or different from the prepared content.
Recipients match by address, since gog rewrites how display names are quoted,
and the stored HTML may sit inside the one `<div dir="auto">` that gog 0.47.1
and later add so the text takes its own direction. The
canonical digest covers the note path, account, actual From/To/Cc/Bcc, subject,
reply target, exact plain text, HTML hash, signature mode, attachment names and
hashes, and flattened links. Volatile Gmail message ids and MIME boundaries do
not affect it.

After a successful readback, Bob stores the durable draft id and a remote
content fingerprint on the note and appends a body-free `gmail.draft` journal
event. Later updates compare the stored fingerprint before replacing a draft;
an external Gmail edit is reported as a conflict. If a create response is
ambiguous, Bob leaves the create marker and will not create a second draft
automatically. A known id is retained in the note or a body-free recovery event
if note persistence fails.

With `-f json`, success is a schema-1 preview document with `status: preview`,
`note`, `draft_id`, `digest`, and `preview`. The preview contains the exact
stored text body, actual envelope, reply target, signature mode, attachment
metadata, and flattened links. Error documents never include message bodies.

```bash
bob gmail draft mail/2026/261106_lease_renewal.md
bob gmail draft 261106_lease_renewal -f json
```

## Outgoing message rendering

The message body excludes any authored `## Notes` section; Notes text never
enters an outgoing message. The remaining body renders to two equivalent
alternatives: an inline-styled HTML fragment and plain text carrying the same
links and references. The supported subset is paragraphs, headings, emphasis,
lists, safe links, blockquotes, and ordinary code. Anything outside that
subset refuses with a kind and a 1-based line number, never with body text.

| Refusal            | Meaning                                                                 |
| ------------------ | ----------------------------------------------------------------------- |
| `task_checkbox`    | Actual `- [ ]` / `- [x]` task syntax, including quoted tasks.            |
| `block_id`         | A trailing `^id` block reference.                                       |
| `inline_field`     | `[key:: value]` or `(key:: value)` inline fields.                       |
| `inline_query`     | Inline `` `= ...` `` or `` `$= ...` `` queries.                         |
| `dataview_fence`   | A ```` ```dataview ```` or ```` ```dataviewjs ```` fence.               |
| `tasks_fence`      | A ```` ```tasks ```` fence.                                             |
| `templater`        | `<% ... %>` syntax.                                                     |
| `unsafe_link`      | A link whose destination is not `http`, `https`, or `mailto`.           |
| `remote_image`     | A remote `![](url)` image, which never loads for recipients either.     |
| `undeclared_embed` | An embed that names no declared attachment.                             |

Refusals apply to active syntax only: the same text inside fenced code, inline
code, or behind a backslash escape stays literal. Raw HTML shows literally
instead of executing. No Dataview, Tasks, or Templater syntax runs, no URL is
fetched, and no wikilink is traversed to gather content.

`[[target|alias]]` flattens to its alias and unaliased links to their visible
target; every flattening is recorded in document order. `![[embed]]` and local
`![](image)` references are accepted only when they name a path from the note's
`attachments` frontmatter (exact vault-relative match, bare filename, `#`/`^`
suffixes ignored, `%20`-style escapes decoded). Accepted embeds become the
transmitted filename — the original vault basename — plus the attachment, never
expanded note content.

Attachments validate as safe vault-relative paths: no absolute paths, parent
references, control characters, symlinks, directories, or missing files. Valid
bytes snapshot once into a private staging directory (mode 0700, files 0600)
that is removed when the operation finishes; transport reads the snapshots so
a later vault edit cannot change the sent bytes. Each attachment records its
vault path, transmitted filename, byte length, and SHA-256 of the snapshot.

## Writing and sending

`bob gmail send NOTE` previews the stored Gmail draft, then sends only that
draft after a confirmation bound to the exact preview. `NOTE` is a compose
note or a saved thread with one Reply section, as a vault-relative path with
or without `.md` or a unique note basename. Like `draft`, it requires the
configured `gmail.writer` host and the isolated `bob-gmail` grant with the
compose scope; on a read-only grant it fails with the
`bob gmail login -s compose` hint instead of sending.

Before preparing a draft, `send` recovers any recorded send attempt. Recovery
can return a state result below, or an error if the outcome remains unknown;
it does not send again. This check also runs with `-d`.

Otherwise, `-d` wins over a confirmation digest: it creates or updates the draft,
reads it back, and prints the intentional preview without sending anything.
That differs from `save -d`, which only prints what it would write. A non-TTY
`send` without `-c` leaves the draft intact, exits 2 with
`confirmation_required`, and prints the exact confirmation command. On a TTY,
Bob shows the colored preview and asks `[y/N]`; anything but an affirmative
answer keeps the draft.

With `-f json`, a draft preview uses a schema-1 document with
`status: preview`, `note`, `draft_id`, `digest`, and `preview`. The
preview carries the account, actual From, ordered To/Cc/Bcc, subject, reply
target (null, or thread id, Gmail message id, RFC Message-ID, and mode),
exact text body, HTML SHA-256, signature mode, attachments (vault path,
transmitted filename, byte length, SHA-256), and flattened links (original
target and resulting text). The Bob Mail plugin renders exactly these fields.
Synthetic examples live under `tests/fixtures/gmail_compose/` and are checked
against CLI output so they cannot drift from the implementation.

Send and recovery results use schema 1 with `status` (`sent`, `already_sent`,
`declined`, or `recovered_to_draft`), `note`, `draft_id`, `thread_id`, and
`sent_message_id`; unavailable identities are null. These state results omit
the preview and digest. `recovered_to_draft` means the prior attempt was not
accepted; a fresh preview is required before another send.

```bash
bob gmail send mail/2026/261105_lease_renewal.md -d
bob gmail send mail/2026/261105_lease_renewal.md -c 0123abcd... -f json
```

The `digest` is the SHA-256 of the canonical preview. Any change to the
message body, recipients, subject, signature mode, attachments, flattened
links, or the remote draft invalidates it: an HTML-only edit invalidates the
digest even when the plain text is unchanged. A stale digest exits 2 with
`confirmation_mismatch` and a re-preview hint. There is no force flag or
environment bypass. A remote edit to the Gmail draft between preview and
confirmation is reported as a conflict instead of being silently replaced.

Immediately before dispatch, Bob rechecks the authored inputs, the attachment
snapshots, and the stored draft contents, then sends with
`drafts send <recorded id>` only — never `gmail send`, `messages send`, SMTP,
or a replacement draft. On success it refreshes the thread through the save
path so the sent card appears, then applies the same watch plan the next
`sync` would under the existing send lock: no second Gmail send, no nested
vault lock, and no extra commit. Sibling watched-thread metadata needed for
multi-thread aggregation is fetched before that lock. Only an enrolled,
currently watched thread changes tasks. A running-session deferral leaves the
Gmail send successful and lets the next `sync` finish the hold; that deferral
does not mark the turn applied. Complete-state recovery retries local
finalization after an acknowledged send and never issues a second send.
A standalone compose note is promoted in place
at its original path: thread identity and the managed block are added, the
submitted body is replaced with the conversation, and compose-only
`status`/recipient/draft fields are removed. Parent, unknown frontmatter,
Notes, and other authored sections survive. A reply send removes only the
exact submitted Reply section and its draft marker. Your sent messages render
as `[!mail-sent]` callouts; inbound mail stays `[!mail]`.

Recovery runs before any render, update, or create whenever a previous send
is unresolved:

- Draft still present → back to draft; the run reports `recovered_to_draft`
  and a fresh preview/confirmation is required before another attempt. A
  recovery run never sends by itself.
- Draft gone and exactly one matching SENT message proven → the send is
  recorded as acknowledged and only local finalization runs.
- Missing, ambiguous, failed lookup, or no matching SENT evidence → reports
  `outcome_unknown`, retains the state, and never creates, updates, or sends
  a replacement.

An acknowledged send whose local refresh fails stays acknowledged and is
retried for local finalization only. A completed operation is a no-op on
repeated send (`already_sent`); starting a later explicit Reply starts a new
operation. Crash recovery restarts from the note's durable frontmatter state,
so it is not lost with one host's cache. There is no atomic
compare-and-send precondition in the Gmail API: an external Gmail edit after
the final readback remains possible, and Bob does not claim exactly-once
delivery across hosts.

Only body-free metadata reaches the journal and errors: note path, draft id,
digest, thread and sent message ids, recipient counts, and a subject hash.
Message bodies, MIME, and attachment bytes never appear there.

## List

`bob gmail list` (or bare `bob gmail`) lists saved mail notes newest first by
parsed `updated` date, with a stable path tie break. Only mail-note
frontmatter is read; bodies never enter the index. The default table shows
the 20 newest threads; `-a` shows all. Table columns are `updated`, `subject`,
`who`, `messages`, and `backlinks`. JSON (`-f json`) carries the same fields
plus the account/thread identity, note path, labelled wikilink, `started`,
`turn`, `turn_since`, `schema_version`, and `ok`, with `total`, `shown`, and
`truncated` for paging. `turn` and `turn_since` are `null` when the note
records no turn.

`backlinks` counts the distinct vault notes whose `[[target]]` wikilink
resolves to the saved note. Aliases (`[[target|label]]`) and heading/block
targets (`[[target#heading]]`, `[[target#^block]]`) resolve through their
target; self-links and links inside fenced/inline code or frontmatter never
count. The count is scanned once per listing.

A configured `gmail.account` scopes the list to that account; otherwise every
locally indexed account is shown. Malformed mail notes are skipped with a
count rather than crashing the list. Duplicate `(account, thread)` identities
are never selected as canonical: they stay visible as diagnostics and are
left out of the threads. The footer reports `N Gmail links waiting · M gone`
once `link` has something to count; it stays absent while both are zero, so it
never claims zero links.

## Link

`bob gmail link NOTE...` scans the named notes for Gmail URLs, saves each
confirmed thread through the save path, and replaces exactly those URL tokens
with labelled wikilinks under the vault maintenance lock. `bob gmail link -a`
scans the whole vault instead: mail notes, `_generated/`, `_conflicts/`,
`.obsidian/`, `.bob/`, `done/`, dot directories, and zorg-era generated logs
(frontmatter `generated_from_zorg: true`) are excluded.

- Replacement is token-exact: only ranges that still hold the same text are
  rewritten, so an edited line is never corrupted. Two identical URLs each
  become links. URLs inside fenced/inline code, frontmatter, and existing
  wikilinks are untouched, as are bare URLs inside titled `[label](url)`
  destinations (untitled `[their words](url)` keeps its label as
  `[[path|their words]]`).
- A URL that Gmail confirms dead (`gone`), refuses, matches `never_save`, or
  never resolves is remembered by URL digest in `.bob/gmail/<host>.jsonl`;
  later sweeps skip it, and the raw URL never enters the journal. Sign-in and
  network failures are not remembered: they set a host-local
  `awaiting_sign_in` marker so the failure is reported once.
- New mail notes and rewritten sources commit together with a
  `gmail: link <subject>` message. A note that changed between planning and
  writing is retried once; a thread whose swaps still cannot land is reported
  as `saved; link needs review` and relinks on the next run.
- `-d` prints the unified diff plus the notes it would create, and touches
  nothing: no lock, directories, notes, images, journals, or commits.
  `-f json` returns the saved threads, swaps (by URL digest, never the URL),
  needs-review entries, failures, and warnings. `-q` prints only failures.
- `gmail.writer` blocks writes on other hosts; dry-run remains available
  there. `link` needs at least one NOTE or `-a`.

Add `-r|--remote` to compare each displayed thread's current Gmail message
count with its saved note; the table shows `+N new`, and JSON adds
`new_messages`. This option makes one Gmail thread read per displayed row and
uses only its message identities. It is uncached so the count is current.

## Search

`bob gmail search [QUERY]` uses Gmail query syntax and defaults to
`in:inbox newer_than:30d` when the query is omitted or blank. It shows 20
threads by default and accepts `-l|--limit` up to 50. The table and JSON rows
include the thread ID, inert subject and counterpart, last date, message count,
a snippet capped at 140 characters, and `saved_note` when the thread is
already indexed in the vault. Search and metadata fetches are read-only.

## Peek

`bob gmail peek URL|ID` resolves a Gmail link without writing the vault and
returns a ready-made preview line. `-f json` includes the resolution verdict,
thread ID, inert subject and counterpart, message count, last date, saved-note
path, and task-safe link label. An unresolvable link returns an
`unresolvable` verdict with a plain message and does not fetch a thread.

Successful previews use a private, metadata-only cache at
`$XDG_CACHE_HOME/bob-cli/gmail/peek.json` (or `~/.cache/bob-cli/gmail/peek.json`)
for ten minutes. It stores no snippets, message bodies, MIME, or attachments.

## Sync

`bob gmail sync` searches `label:<gmail.sync.label>` (default `label:bob`,
falling back to `gmail.pull.label`), up to 50 new threads by default and at
most 500 per invocation. `-l` caps new filings only; `0` skips new-candidate
search while still scanning, adopting, and reporting watchers normally.
Already enrolled threads and locally adoptable threads never consume filing
slots and never receive candidate metadata or body fetches. Each unseen
thread is saved through the normal save path, then one `- [ ] #task` row
linking its mail note is appended under `## Tasks` in the area or project
named by a routing label, or in `gmail.sync.inbox` when none or an invalid
routing label is present. New task rows carry no freshness stamp. If the
inbox file is missing, Bob creates it as an inbox area note. A missing
`sync.label` falls back to `pull.label`, then `bob`; an explicitly empty
label disables the command as a setup error.

Enrollment identity is `(account, Gmail thread id)`. The `.bob/gmail/*.jsonl`
`gmail.filed` and `gmail.adopted` events prevent repeat filing; legacy
`gmail.pulled` events still enroll a thread. Enrollment is durable: removing
the Gmail label, or closing, moving, merging, or deleting the task, never
un-enrolls a thread. An open task that already links an unenrolled mail note
is adopted with no new row, including after a lost journal append and even
when the label is gone. Reopening or linking from a new open task restores
membership without needing the label again. `pulled` counts only durable
`filed`/`would_file` rows, including filed route problems; fetch and
planning failures never count. `watching` counts distinct enrolled threads
with open watchers from the revalidated apply snapshot plus this run's
filings and adoptions. `awaiting` counts those currently watched threads
whose open tasks still carry an exact `#awaiting_reply` token. Unwatched
enrollments and unsuccessful new enrollments never count. `-d` reads and
plans without writing, taking no guard or vault lock. `-d` is allowed on a
non-writer host: the writer pin is live-only. `-r` is the remaining allowance
for waiting (default 120 seconds): guard contention, retry backoff, and
vault-lock waits. Ordinary pacing (the 200 ms minimum gap between Gmail
requests) always applies, including at `-r 0`, and never consumes the budget.
`-r 0` means one attempt per operation with no contention, backoff, or
lock-wait sleep. By default, sync leaves Gmail unchanged and uses read-only
access. The opt-in archive path below is the only message-label mutation:
it removes `INBOX` and adds the receipt label to exact saved message IDs.
It never marks read, deletes, or changes unrelated labels. `pull` remains a
hidden alias with the same behaviour.

### Archive on close

Archiving is off until `gmail.sync.archive: on` is set and the approved
writer has the Gmail modify grant from `bob gmail login -s modify`. The
Google grant is broad; Bob's implementation uses an exact command fence for
message label changes and does not send or permanently delete mail. Compose
still follows its separate draft and confirmation workflow.

Archive eligibility requires an enrolled thread that still has its Gmail
enrollment label, at least one linking task closed as `[x]`, `[X]`, or `[-]`,
and no open or unknown linking task. The strict scan includes `done/`,
refuses incomplete or ambiguous mail evidence, and holds merge-retired
sources for review with no human row. The baseline needs a live run on the
writer that has the modify grant: the first such run after enabling records
enablement and open observations; only a later observed open-to-closed
transition archives automatically. A thread already closed at the baseline is
historical backlog and needs a matching preview digest. Reopening and closing
entirely between sync observations cannot be detected. An `archive_label`
rename re-baselines: the next live run records a new enablement, old-epoch
closes project as backlog and need `-c`, and owned receipts keep their old
identity for let-go.

Bob freezes the message IDs already saved in the mail note. If an Inbox reply
is present but unsaved, the whole thread stays in Inbox and one `kept`
(`unsaved`) outcome is recorded, consuming the close; a reopen re-arms it.
Removing the enrollment label is a per-thread opt-out; Bob reports that case
in JSON without a human row. If you already archived a thread yourself, Bob
records one `kept` (`manual`) that consumes the close and adds no receipt.
Policy exclusions (Spam, Trash, Drafts, `never_save`) and gone mail are a
one-time `kept` that consumes the close; they are not held. Merge holds stay
silent.

The Gmail receipt label records Bob-owned archives and supports crash
recovery. A new reply or your manual Move to Inbox removes the receipt only;
Bob then lets go (removing the receipt label) and never archives that close
again. Reopening a task does not move mail: the report gives a typed restore
hint, and restore stays manual: Move to Inbox in Gmail. There is no
`bob gmail unarchive` command and no automatic restore; `restored` is a
reserved JSON action that is never emitted. Present no deferred surface as
shipped: no `unarchive`, no `gmail_archived` field, no `recent_archive`
listing, no chips, and no notifications.

Use `-d` to preview without writing, `-c DIGEST` to confirm the exact
historical backlog or a batch above `archive_max`, and `-n` to skip archive
planning for one run. A pause shows every candidate thread, its `decision`
(`would_archive` or `kept:<reason>`), and the digest, archives none, and
exits 1. A supplied digest that no longer matches the current saved IDs,
labels, or tasks makes no archive writes. Archive failures do not undo
completed filing or watch work; `invalid_grant` stops remaining Gmail
mutations. `-q` still shows archive and let-go rows because those changes
matter to the Inbox, but it never prints the digest for a live successful run.

The journal at `.bob/gmail/<host>.jsonl` records body-free enablement,
observations, close generations, and terminal outcomes. No task prose,
subject, body, or token is stored there. Events are `gmail.archive_enabled`
(account, enabled, epoch, label), `gmail.archive_observed` (open witness),
`gmail.archive_closed` (frozen saved IDs, note, tasks, label), `gmail.archived`
(`message_ids`, label, `recovered`), `gmail.kept` (`reason`), `gmail.let_go`
(`message_ids`, label), and `gmail.restored` (reserved, never written).
Reasons are `eligible`, `backlog`, `unsaved`, `consent_removed`, `policy`,
`gone`, `manual`, `replied`, `manual_return`, and `recovered`. Actions are
`would_archive`, `archived`, `kept`, `let_go`, `paused`, and reserved
`restored`. Hints name the missing-modify grant and the confirm digest;
errors are typed `{kind, message, hint?}` with exit 2 for setup (`modify_grant`)
and auth, and exit 1 for paused, `confirm_mismatch`, and archive failure.

A thread whose routing label does not resolve is still filed to the inbox
with a managed `GMAIL ROUTE` child that names the offending label, the
resolver's explanation, and its hint; the first such filing exits 1.
Each later run recomputes that inbox row: a resolving route moves the task
through the task-move engine with no freshness stamp and appends
`gmail.refiled`; a persisting problem updates the warning and reports
`first_seen: false` without failing the run; leftover unrouted labels drop
the warning. A failed refile leaves the row and its warning in the inbox,
adds a warning, and exits 1. A refile repairs links to the moved block
across the vault, as Ctrl+Shift+M does. Rows outside the inbox never move.
A routing label that names a different note is reported once
(`gmail.route_conflict`). Each live run
lists account labels once, including `-l 0`. Failures carry their body-free
`kind` and `message` in warnings, and human and quiet rows show the route
label, explanation, and hint for filed route problems while distinguishing
remote failures with no durable row.

JSON schema 1 keeps pull's `ok`, `dry_run`, `pulled`, `results`, and
`warnings`, and adds `watch`, `problems`, and
`summary: {watching, awaiting, archived}`. Each `watch[]` row carries
`thread_id`, `note`, `tasks` (each `path`, 1-based `line`, and nullable
`block_id`), `turn`, `turn_since`, `action`, and optional `nudge_on`. Actions
are `held`, `lifted`, `bounced`, `resurfaced`, `deferred_running`,
`refreshed`, and `unchanged`. A first-seen failure keeps `ok: false`: the
`results[]` row is present with `status: problem` and the body-free warning
names the kind. A missing or trashed thread is not an action: it is reported
once through a `gmail.gone` journal event plus a human and warning row.
`results[]` rows carry `thread_id`, `note`, `link`, `status`, `task_file`,
and `route`; `problems[]` rows carry `thread_id`, `note`, `label`, `kind`,
`message`, `hint`, and `first_seen`. Result statuses are `filed`,
`would_file`, `adopted`, `already_filed`, `already_pulled`, `problem`,
`refiled`, and `would_refile`. Archive rows add `decision`, and only dry-run
and paused runs carry `archive_digest`; `-n` sets `archive_skipped`.
`-d` is allowed on a non-writer host: the writer pin is live-only. Exit 0 on
success, no-op, or a quiet contention skip; 1 on a runtime failure, deferred
work, a first-seen route problem, a paused batch, `confirm_mismatch`, or an
archive failure, or a quiet busy-skip once live runs have kept skipping for
more than two hours with no successful run in between; 2 on setup (empty
label, writer, bad `-l`, `modify_grant`) and auth.

```bash
bob gmail sync -d
bob gmail sync -l 5 -f json
bob gmail sync -l 0
bob gmail sync
bob gmail sync -c DIGEST
```

The [phone filing guide](guides/file-email-from-your-phone.md) explains how
to label the original conversation with `bob` plus one routing label. A
`+bob` forwarding filter remains available only as a separate alternative
that files the forwarded copy, not the original.

## Watch

The link is the wire. Every open `#task` whose own line, or a direct
link-only child bullet, links a saved mail note watches that thread — a
Gmail Task ([glossary](reference/glossary.md)). The
watch follows Ctrl+Shift+M moves and Task Merges. Enrollment is durable:
removing the Gmail label never ends it. Fetching stops when every linking
task is closed, cancelled, or deleted; a later open link restores
membership without the label. A Gmail Task's thread opens in the browser
with Cmd+Enter ([Mail link marks](mail-link-marks.md)).

Each `bob gmail sync` run, including `-l 0`, refreshes currently watched
threads through the same coordinated read used by discovery. A first watch
folds the whole thread; later runs fold after the latest applied
`gmail.turn` marker. Bob changes a task only when a new counted move
arrives. Between messages the line is Bryan's: polling that sees no new
move consumes the turn without rewriting the task, so a hand-edited date or
tag survives. Unknown order or direction changes no task and advances no
marker. Automated and ignored messages may refresh the mail note and do not
hold or lift. Events and recovery evidence contain no body, snippet,
subject, or task title.

The stored hold is an exact `#awaiting_reply` tag plus one `[scheduled::]`
nudge date. Display draws a future-dated `[?]` as a hollow paper plane
([Blocked-reason marks](blocked-marks.md)). Reconcile treats that date as an
ordinary scheduled hold and never reasserts the tag
([Task Status Hooks](task-status-hooks.md#derived-blocked-status)).
[Awaiting A Gmail Reply Is A Tagged Schedule Hold](develop/decisions.md#gmail-reply-wait-is-a-tagged-hold)
records why.

Examples use `BOB_NOW=2026-11-03` and `gmail.sync.reply_wait_days: 3`.

### Hold

A new outgoing counted send holds every open linking task. The nudge date is
the send's local calendar date plus `reply_wait_days`, not the sync date.
Bob inserts exact `#awaiting_reply` after `#task`, or after `#prj_is_blocked`
when that token is present, and replaces the sole `scheduled` field. A
future nudge flips an open Ready, Next, or Pending box to `[?]`. An
already-arrived date keeps the original box. Recurring rows (`repeat`) and
duplicate `scheduled` fields are refused with a diagnostic. Freshness
fields are never touched.

```bob-task
- [?] #task #awaiting_reply [[mail/2026/261103_lease_renewal|✉️ Lease renewal · Ada]] [scheduled:: 2026-11-06]
```

A later outgoing send replaces that nudge. Changing `reply_wait_days` never
reschedules an existing hold.

### Lift

A new incoming counted move or bounce removes the exact awaiting token and
then removes `scheduled` only if that token was present. `[?]` recovers to
`[ ]` only with no `dependsOn` field, no exact `#prj_is_blocked`, and no
remaining future schedule. After the hold date is gone, an eligible Ready
row runs the existing `[unblocked::]` writer
([Freshness §5](freshness.md#5-who-stamps), vectors UB16–UB18). WRITE skips
stay: NEW, out of freshness scope, an existing scheduled tickler, or a Next
or Pending landing. An unheld Next or Pending task is unchanged. Neither
hold nor lift stamps, renames, closes, creates children, changes
priority or keeps, or restores a pre-hold lane.

When nobody replies, the nudge date arrives and the still-tagged Ready row
walks as an ordinary scheduled tickler. The no-reply pill is display-only
([Mail link marks](mail-link-marks.md)).

### Running sessions

A hold whose resolved identity has a live Task Link under today's open timed
Pomodoro is deferred. Planned placeholders, closed entries, struck or
dropped links, and mentions do not defer. An ambiguous running ledger
reports and defers affected holds rather than guessing. The thread marker
does not advance for that deferred hold; other eligible tasks on the same
thread still receive the move. The next `sync` after the session closes
finishes the hold. Catching up after a deferral never rewrites targets the
earlier run already applied.

### Several threads, several tasks

One effect is computed per task after gathering every linked watched
thread, not in thread iteration order. Apply nothing without a new counted
move. A new incoming move or bounce lifts even when other threads still
wait. Hold only when every linked watched thread is known to be waiting,
with the nudge date from the newest send. An unknown or unfetched sibling
prevents claiming that all threads await replies. Several tasks on one
thread are independent: a running deferral on one does not block the
others.

### Reports

Human watch rows use inert title and who from mail metadata:

| `action` | Human row |
| --- | --- |
| `held` | `✉ awaiting reply` plus `(nudge YYYY-MM-DD)` |
| `lifted` | `✉ replied` |
| `bounced` | `✉ bounced` |
| `resurfaced` | `✉ resurfaced` |
| `deferred_running` | `✉ deferred (running task)` |
| `refreshed` | `✉ refreshed` |
| `unchanged` | omitted from human output |

Quiet (`-q`) suppresses ordinary rows and keeps failures, warnings, and
running deferrals. A changed watch, a first-seen gone thread, or a new
filing suppresses the header's `nothing new` suffix. Invalid grant aborts
all writes. Isolated permanent errors and exhausted retries preserve
successfully prepared independent work and report failure.

## Attachments

Mail notes list downloadable attachments in conversation order with a
thread-wide number. `bob gmail attach NOTE N` fetches one selected file and
rewrites only that attachment row. Its vault path is
`file/mail/<thread id>/<sanitized filename>`; conflicting names get a short
content-hash suffix. Bob refuses files above 25 MiB, checking both Gmail's
declared size and the received bytes. The origin is recorded in
`.bob/attachments/<host>.jsonl` with `via: "gmail"` and the canonical Gmail
thread URL as its source.

`-d` previews the path and size without writing. Repeating an import whose
file and origin are already present is a no-op. Attachment bytes stay in
memory until the vault write and never appear in logs or journals.

```bash
bob gmail attach mail/2026/lease_renewal.md 1 -d
bob gmail attach mail/2026/lease_renewal.md 1
```

## Try it

1. `bob gmail save <URL> -f json` saves one thread and prints its note path.
2. `bob gmail list` shows the saved thread with its backlink count.
3. Add `[[<note path>|<label>]]` to a task note, then `bob gmail list` again
   to see the count rise.
4. Paste a Gmail URL into a task note and run
   `bob gmail link <note> -f json`: the URL becomes the labelled link.
5. `bob gmail link -a -d` previews a backfill of old URLs without writing.
6. `bob gmail list -f json` shows the same rows with identities and links.
7. `bob gmail sync -d` previews the configured label sync; `bob gmail sync`
   files it into the matching area or project, or `gmail_inbox.md`.
8. `bob gmail attach <note path> 1 -d` previews the first attachment import.
9. `bob gmail doctor` checks decoder health and duplicate identities.
10. To send a real message, follow
    [Send email from Obsidian](guides/send-email-from-obsidian.md): `new`,
    `draft`, then a confirmed `send` to an address you choose.

## Doctor

`bob gmail doctor` checks gog, the stored `bob-gmail` client, the account, the
token (one hydrated `search` page), IDs-only listing, the granted scopes, the
config, the writer pin, and journal readability. Rows look like
`bob gcal doctor`:

- `gog`: version and program path.
- `client`: stored `bob-gmail` OAuth client and its Cloud project.
- `account`: the resolved email and its Gmail grant.
- `token`: `refreshes`, or `invalid_grant` with a login hint. Testing-mode
  consent screens issue 7-day tokens; a Google password change revokes Gmail
  tokens; a token unused for 6 months expires.
- `thread_ids`: IDs-only `threads.list` of the sync-label query
  (`label:<label>`), or `in:inbox` when sync is disabled. Skipped when there
  is no account or the token row failed.
- `scopes`: the granted `readonly`, `compose`, and `modify` combination.
- `archive`: on/off, resolved receipt label, `archive_max`, and distinct
  archived and kept outcomes since Monday midnight in the configured
  timezone. A missing modify grant while archive mode is on shows the login
  hint even when the journal also warns; the row stays visible as skipped
  when the account or grant cannot be checked.
- `config`: folder, body mode, the sync label and inbox after fallbacks
  (`sync disabled` when the label is empty), the route prefixes (`none` in
  no-prefix mode), and the reply wait in days.
- `routes`: a warning, never a failure. It reads the account's label list once
  and lists every label under a configured prefix that doesn't resolve, one
  `label → kind · hint` line each. When all resolve it shows the routing-label
  count. It is skipped in no-prefix mode, which has no routing labels to audit,
  and when there is no signed-in account or the token fails.
- `writer`: this host, or the pin mismatch.
- `sync`: local only. This host and the writer pin, the last successful run
  as a relative time (`never` until one completes), the open route-problem
  count, and the watching and awaiting counts. A last success older than two
  hours is a warning. It runs even when sign-in checks fail.
- `journal`: `.bob/gmail/*.jsonl` tracked and union-merged.
- `journal_malformed`: malformed lines skipped with a count.
- `gmail_urls`: every Gmail URL in vault notes with resolvable, refused, and
  unknown-token counts. Local only: it runs even when sign-in checks fail.
- `gmail_duplicates`: duplicate `(account, thread)` identities with a count.
  A warning, never a failure: listing skips duplicates instead of picking one.

Exit 1 when any check fails. `-f json` prints `schema_version: 1`, `ok`, the
host, and the typed checks.

## Where it runs

Any logged-in host may read Gmail. Save, sync, and attachment writes run only
on `gmail.writer` when that key is set; the docs recommend the Mac. On the Mac,
cron runs `bob gmail sync -q` hourly at `:07`; see
[Mac scheduled maintenance](vault-git-sync.md#mac-scheduled-maintenance). Agent
hosts stay read-only: grant modify only to the approved writer. `doctor`
reports granted scopes, archive state, the writer pin, and the current host.
Sync dry-run (`-d`) is allowed on a non-writer host.

Live sync holds a host-local per-account guard at
`$XDG_STATE_HOME/bob/gmail/sync-<digest>.lock`. Each live run records
`last_attempt_at` and, when it completes, `last_success_at` in
`$XDG_STATE_HOME/bob/gmail/sync-state.json` (including first-seen route
problems). A quiet busy-skip exits 1 once live runs have kept skipping for
more than two hours with no successful run in between. Dry-run writes
neither file. If a scoped commit or journal
append fails, body-free recovery evidence at
`$XDG_STATE_HOME/bob/gmail/sync-recovery-<digest>.json` lets a later live
run on this host finish that work before new enrollment. The digest keys
the file to this account and vault; the evidence never ships with the
vault.

## Exit status

0 success, 1 runtime failure, 2 usage or setup error. Config errors name the
key (for example `gmail.account`). `summary.archived` counts live `archived`
rows. Exit 2 for setup (`modify_grant`) and auth (`invalid_grant`); exit 1
for paused batches, `confirm_mismatch`, and any archive failure that keeps
filing and watch rows. A quiet Gmail sync busy-skip is 0 unless live runs
have kept skipping for more than two hours with no successful run in between,
in which case it is 1.

## JSON output

Every `-f json` output carries `schema_version: 1` and `ok`. Errors are typed
`{kind, message, hint?}` on stdout; human errors go to stderr with a dim hint.
[Compose notes](#compose-notes), [Gmail drafts](#gmail-drafts), and
[Writing and sending](#writing-and-sending) describe the compose documents.
`bob gmail sync -f json` prints the schema-1 sync document (`ok`,
`dry_run`, `pulled`, `results`, `warnings`, `watch`, `problems`, and
`summary`) plus additive `archive[]`, `archive_hints[]`, `archive_enabled`,
`archive_label`, `archive_max`, and `archive_skipped`. Archive rows carry
`thread_id`, `note`, `action`, `reason`, `decision`, exact sorted message IDs,
and task path/line/block-ID locators, never task text. Only dry-run and paused
runs add `archive_digest`. Reopen advice is a typed hint, not a watch action.
`restored` is reserved and never emitted. `watch[]` and
`summary.{watching,awaiting,archived}` are specified under [Watch](#watch).
See [JSON outputs](reference/json-outputs.md) for the field list and status
contract.

## Security

- Tokens never appear in argv, logs, journals, diagnostics, caches, or JSON
  errors. Client JSON travels to gog over stdin.
- Bodies and raw MIME appear only in mail notes. Attachment bytes stay in
  memory until Bob writes the imported vault file; they never appear in argv,
  logs, the Gmail journal, or JSON errors. Search shows only a capped, inert
  snippet. Search, peek, and list `--remote` use only headers, labels, dates,
  and snippets. gog's `gmail thread get` has no metadata-only mode, so its
  response also carries body parts; Bob discards them. The preview cache
  stores no snippets. Reads never pass `--sanitize-content` or
  `--wrap-untrusted`.
- `gmail.compose` also permits sending. Bob never sends without a Gmail draft
  plus a digest-bound confirmation, and keeps that grant off agent hosts.
- `gmail.modify` is broader than label archiving. Bob requests it only for the
  approved writer and fences the archive path to the receipt/`INBOX` label
  pair; sync does not use it to send, delete, mark read, or modify other
  labels.
- Tests use a fake gog and temp vaults. Committed fixtures are synthetic.

## Environment

- `BOB_DIR`: vault root; defaults to `~/bob`.
- `BOB_CONFIG_FILE`: config file; defaults to `~/.config/bob/config.yml`.
- `BOB_GMAIL_GOG`: gog executable overriding `gmail.gog` and `gog` on `PATH`.
- `BOB_GMAIL_BROWSER`: browser for sign-in (otherwise automatic).
- `BOB_GMAIL_ASSUME_TTY`: `1`/`true` treats stdin as a TTY for prompts.
- `BOB_GMAIL_CONSENT_SECS`: consent window override (tests).
- `XDG_CACHE_HOME`: cache root; defaults to `~/.cache`.
- `XDG_STATE_HOME`: host-local state root; defaults to `~/.local/state`.
  Sync stores its guard lock and recovery evidence under
  `$XDG_STATE_HOME/bob/gmail/`.

## Known limits

Confirmed sends need the compose grant on the writer host, one explicit
confirmation per preview, and a Gmail draft that still exists;
outcome-unknown states require Bryan to inspect the draft and SENT folder
before retrying.

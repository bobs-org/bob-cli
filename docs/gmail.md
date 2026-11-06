# Bob Gmail

`bob gmail` saves Gmail threads you choose as inert, append-only mail notes in
the vault, swaps Gmail URLs in notes for readable links, and creates local
compose and reply notes. It also pulls labelled threads into inbox tasks and
imports selected attachments. This page covers one-time setup through
[gog](https://github.com/openclaw/gogcli) (≥ v0.44.0), config, login/logout,
doctor, saving, linking, listing, search, previews, label pulls, attachment
imports, and compose-note creation. `new` and `reply` write local notes without
sending mail; sending remains later work. Bare `bob gmail` lists saved threads
offline.

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
- [Outgoing message rendering](#outgoing-message-rendering)
- [List](#list)
- [Link](#link)
- [Search](#search)
- [Peek](#peek)
- [Pull](#pull)
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
bob gmail link NOTE... [-a|--all] [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-q|--quiet]
bob gmail login [-a|--account EMAIL] [-c|--credentials FILE] [-f|--format human|json] [-m|--manual] [-s|--scope readonly|compose]
bob gmail logout [-a|--account EMAIL] [-f|--format human|json] [-y|--force]
bob gmail new [-b|--bob-dir DIR] [-B|--bcc LIST] [-c|--cc LIST] [-f|--format human|json] [-s|--subject TEXT] [-t|--to LIST]
bob gmail peek URL|ID [-f|--format human|json]
bob gmail pull [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-l|--limit N] [-q|--quiet]
bob gmail reply NOTE [-a|--all] [-b|--bob-dir DIR] [-f|--format human|json]
bob gmail save URL|ID|NOTE... [-b|--bob-dir DIR] [-B|--body full|headers] [-d|--dry-run] [-f|--format human|json] [-P|--parent ROUTE] [-q|--quiet] [-R|--rerender]
bob gmail search [QUERY] [-f|--format table|json] [-l|--limit N]
```

Running `bob gmail` with no subcommand lists saved threads, exactly like the
explicit `bob gmail list`. Listing is offline and read-only by default; `-r`
fetches live message counts for the rows shown. `save` reads Gmail and writes
selected threads to the vault. `link` finds Gmail URLs in notes, saves each
confirmed thread, and replaces those URL tokens with labelled wikilinks.
`pull` reads the configured label and files saved threads as inbox tasks
without changing Gmail labels. `attach` reads one listed attachment and writes
its bytes into the vault. `new` and `reply` write local Markdown notes without
login or network access. `doctor`, `search`, and `peek` are read-only. `login`
and `logout` are excluded from the command catalog because they need a terminal
and handle credentials; `new` and `reply` are excluded because the compose
flow is owned by Bob Mail.

## How it works

Bob talks to Google only through `gog`. Read flags go before the subcommand
and never include tokens:

`--no-input --json --color never --readonly --client bob-gmail --account <email> --gmail-no-send --enable-commands gmail.thread.get,gmail.get,gmail.messages,gmail.search,gmail.url,gmail.labels.list`

Attachment bodies use a narrower Discovery read (`api call gmail v1
gmail.users.messages.attachments.get --params … --scope
https://www.googleapis.com/auth/gmail.readonly --no-cache`) with
`--enable-commands api.call,api.gmail.users.messages.attachments.get`.
The legacy `gmail attachment` command is never used: it writes a cache file.

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
- `body` is `full` (default) or `headers`. `save` honors it; headers
  mode keeps only the snippet plus a "(body not saved)" marker.
- `folder` is the vault folder for mail notes. Default `mail`.
- `link_label` templates the task link. Default `✉️ {subject} · {who}` under
  the `subject_who` decision. It must contain `{subject}`.
- `never_save.from` lists addresses or `@domains`; `never_save.labels`
  lists Gmail label names. `save` refuses matching threads with a reason.
- `pull.label` unset or empty disables pull. Default `bob`; inbox default
  `gmail_inbox.md`. See the [phone filing guide](guides/file-email-from-your-phone.md).
- `pull.inbox` must be a safe relative path inside the vault. If it is
  missing, Bob creates it with `parent: "[[inbox]]"`, `type: "[[area]]"`,
  `ready_cap: off`, and a `## Tasks` section.
- `signature` is `gmail` (default) or `none`; the later send flow honors it.
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
images, and newly created `mail.md` / `mail.base` hubs. Save journals are
appended after the commit (or after durable writes in a non-Git vault).
`gmail.writer` blocks writes on other hosts; dry-run remains available there.

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

## List

`bob gmail list` (or bare `bob gmail`) lists saved mail notes newest first by
parsed `updated` date, with a stable path tie break. Only mail-note
frontmatter is read; bodies never enter the index. The default table shows
the 20 newest threads; `-a` shows all. Table columns are `updated`, `subject`,
`who`, `messages`, and `backlinks`. JSON (`-f json`) carries the same fields
plus the account/thread identity, note path, labelled wikilink, `started`,
`schema_version`, and `ok`, with `total`, `shown`, and `truncated` for
paging.

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
`new_messages`. This option makes one metadata-only Gmail read per displayed
row. It is uncached so the count is current.

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

## Pull

`bob gmail pull` searches `label:<gmail.pull.label>` (default `label:bob`),
up to 20 threads by default and at most 500 per invocation. Each thread that
has not already been pulled is saved through the normal save path, then one
`- [ ] #task` row linking its mail note is appended under `## Tasks` in
`gmail.pull.inbox`. New task rows carry no freshness stamp. If the inbox file
is missing, Bob creates it as an inbox area note. A missing or empty
`pull.label` disables the command.

Pull identity is `(account, Gmail thread id)`. The `.bob/gmail/*.jsonl`
`gmail.pulled` events prevent repeat filing; Bob also checks for an existing
task link so it can recover if an inbox write succeeded before its journal
append. `-d` reads and plans the save and inbox task without writing. The
label stays in Gmail: pull uses read-only access and never unlabels, archives,
marks read, or changes a message.

```bash
bob gmail pull -d
bob gmail pull -l 5 -f json
bob gmail pull
```

The [phone filing guide](guides/file-email-from-your-phone.md) explains how
to apply the label in Gmail and set up a `+bob` forwarding filter.

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
7. `bob gmail pull -d` previews the configured label pull; `bob gmail pull`
   files it into `gmail_inbox.md`.
8. `bob gmail attach <note path> 1 -d` previews the first attachment import.
9. `bob gmail doctor` checks decoder health and duplicate identities.

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
- `gmail_urls`: every Gmail URL in vault notes with resolvable, refused, and
  unknown-token counts. Local only: it runs even when sign-in checks fail.
- `gmail_duplicates`: duplicate `(account, thread)` identities with a count.
  A warning, never a failure: listing skips duplicates instead of picking one.

Exit 1 when any check fails. `-f json` prints `schema_version: 1`, `ok`, the
host, and the typed checks.

## Where it runs

Any logged-in host may read Gmail. Save, pull, and attachment writes run only
on `gmail.writer` when that key is set; the docs recommend the Mac. Agent
hosts stay read-only:
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
- Bodies and raw MIME appear only in mail notes. Attachment bytes stay in
  memory until Bob writes the imported vault file; they never appear in argv,
  logs, the Gmail journal, or JSON errors. Search shows only a capped, inert
  snippet. Peek and list `--remote` fetch headers and dates only. The preview
  cache stores no snippets. Reads never pass `--sanitize-content` or
  `--wrap-untrusted`.
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
- `XDG_CACHE_HOME`: cache root; defaults to `~/.cache`.

## Known limits

Sending compose and reply notes as Gmail drafts and dedicated task
linking remain later work.

# Bob Gkeep

`bob gkeep` drains the Google Keep inbox into Obsidian tasks. Every Keep inbox
note becomes a task in `gkeep_inbox.md`, and each note is archived in Keep
only after its current content is provably in the vault. Typical order:
`bob gkeep` (or `bob gkeep list`) to see Keep and the vault side by side,
`bob gkeep pull -d` to preview the exact Markdown, `bob gkeep pull -n` to
write without archiving, then `bob gkeep pull` to write and archive.
`bob gkeep login` is the one-time setup; `bob gkeep doctor` diagnoses the
setup. Tests never touch live Keep: set `BOB_GKEEP_ADAPTER` to a fake adapter.

## Contents

- [Commands](#commands)
- [How it works](#how-it-works)
- [Setup and rollout](#setup-and-rollout)
- [Configuration](#configuration)
- [List](#list)
- [Pull](#pull)
- [Rendering and escaping](#rendering-and-escaping)
- [Marker, ledger, and journal](#marker-ledger-and-journal)
- [Exit status](#exit-status)
- [JSON output](#json-output)
- [Security](#security)
- [Known limits](#known-limits)
- [Environment](#environment)

## Commands

```bash
bob gkeep [-a|--all] [-b|--bob-dir DIR] [-f|--format table|json] [-s|--source both|keep|vault]
bob gkeep list [-a|--all] [-b|--bob-dir DIR] [-f|--format table|json] [-s|--source both|keep|vault]
bob gkeep pull [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-i|--id REF]... [-p|--include-pinned] [-S|--include-shared] [-l|--limit N] [-n|--no-archive] [-C|--no-commit] [-q|--quiet] [-R|--no-ref]
bob gkeep login [-e|--email EMAIL]
bob gkeep doctor [-b|--bob-dir DIR] [-f|--format human|json]
```

Running `bob gkeep` with no subcommand runs `bob gkeep list` with the same
options, so `bob gkeep -s vault` works.

## How it works

Keep has no official read/write API for personal notes, and Takeout exports
are manual snapshots, so `bob gkeep` talks to Keep through the unofficial
`gkeepapi` Python library. The pinned adapter (`scripts/gkeep_adapter.py`)
speaks a small JSON protocol over stdin/stdout (see [Pull](#pull) for the
ops); the Rust side spawns it with `uv run --script`, which fetches a pinned
Python and pinned dependencies on first run. `BOB_GKEEP_ADAPTER` replaces that
spawn with any executable speaking the same protocol, which is the test hook.

The pipeline is: snapshot Keep, scan the vault for `%%gkeep:…%%` markers
(the ledger) plus the append-only journal, classify each note
(new / pending / revised / skipped), render new and revised notes as Markdown
blocks, append them to the target note with a durable atomic write, re-read
and parse-verify the write, commit when the vault is a Git worktree, and only
then archive each note in Keep with a content guard: the adapter re-reads the
note and archives it only when its current content still equals what Rust saw.
Notes edited in Keep during a pull stay in Keep; the next pull writes the
revision. Nothing is ever deleted from Keep.

The adapter runs in its own process group so a timeout can kill the whole
tree. `BOB_GKEEP_PARENT_PID` carries the Rust parent pid to the adapter;
a watchdog thread exits the adapter when the parent disappears, so Ctrl-C
cannot orphan it. `internal` adapter errors include the adapter's stderr
tail (last 20 lines).

## Setup and rollout

Roll out against the live account in this order. Bryan runs these steps; no
agent ever runs them.

1. Add the `gkeep:` config (see [Configuration](#configuration)).
2. `bob gkeep login`: reads a Google sign-in cookie from a hidden TTY prompt
   (or from stdin when stdin is not a TTY), exchanges it through the adapter
   for a master token, stores the token via `token_store_command`, then reads
   the token back and checks Keep reachability.
3. `bob gkeep doctor`: walks the checklist (config, account, token, adapter,
   Keep, target note, plus git) and reports what is wrong.
4. `bob gkeep`: shows both inboxes and what `pull` would do.
5. `bob gkeep pull -d`: previews the exact Markdown a pull would write.
6. `bob gkeep pull -n`, then inspect the vault.
7. `bob gkeep pull`: writes, verifies, commits, then archives in Keep.

Note: if `pass` already holds a `gkeep_oauth_token` entry, it is probably a
sign-in cookie, not a master token. `doctor` says which shape the stored
value has; `token_command: pass show gkeep_oauth_token` works as the token
command only when that entry already holds an `aas_et/…` token.

## Configuration

`bob gkeep` reads `~/.config/bob/config.yml` (override with
`BOB_CONFIG_FILE`), resolved by `config::config_path()`:

```yaml
gkeep:
  email: bryanbugyi34@gmail.com # required for Keep access
  token_command: pass show gkeep/master_token # default; prints the aas_et/… master token
  token_store_command: pass insert -m -f gkeep/master_token # default; `login` pipes the token on stdin
  device_id: 3f9c0a1b2c3d4e5f # optional hex; default derived from email
  target: gkeep_inbox.md # optional; vault-relative; default gkeep_inbox.md
  timeout_secs: 300 # optional adapter timeout
```

Parsing follows the `highlights` pattern exactly: a missing file gives the
defaults, unknown keys are ignored, and invalid YAML is an error. Resolving
validates: `email` must contain `@`; `device_id` must be 1–16 hex digits;
`target` must be vault-relative and end in `.md` (a `..` component is
rejected); `timeout_secs` must be greater than 0.

The default device id is `hex(sha256("bob-gkeep-device:" +
lowercase(email)))[..16]`, so every host presents one stable Android device
id with zero config; `gkeep.device_id` overrides it.

`token_command` and `token_store_command` run with `sh -c` (same as
`highlights.pre_scan_hook`) with stdin/stderr inherited, so `pass` can use
pinentry. The token is the first non-empty stdout line, trimmed. Token shape:
`aas_et/…` is a master token; `oauth2_4/…` is a sign-in cookie and errors
with the hint to run `bob gkeep login`; anything else warns and is tried
anyway. Tokens never appear in argv, logs, errors, or JSON.

## List

`bob gkeep list` shows Keep inbox notes and `gkeep_inbox.md` tasks side by
side in two sections: a Keep table, then the vault tasks. `-s keep` shows
only Keep (still needs the network); `-s vault` shows only vault tasks with
no network. `-a, --all` also lists archived Keep notes and done/canceled
vault tasks.

Human tables shorten the text column so each row fits the terminal width:
`COLUMNS` when it is a positive integer, otherwise 100 columns. The Keep
table has `REF`, `AGE`, `KIND`, `STATE`, and `NOTE` columns. `REF` is the
first 7 hex digits of `sha256(id)`; it is shown in `list` and accepted by
`pull -i`. States, oldest first:

| State | Meaning |
| --- | --- |
| `new` | Not in the vault; a pull writes it, then archives it |
| `pending` | Already in the vault with the same content; a pull archives only |
| `revised` | In the vault with older content; a pull writes the revision, then archives |
| `empty` | No title, text, items, or attachments; skipped |
| `pinned` | Pinned; skipped unless `--include-pinned` or selected with `-i` |
| `shared` | Shared with collaborators; skipped unless `--include-shared` or selected with `-i` |
| `archived` | Already archived (only with `--all`) |

Keep `NOTE` hints: `+N lines` for extra text lines (`+1 line`
singular), `☐ n ☑ m` for list items, `📎 n` for attachments (images,
drawings, audio, and `other` files all count).

The vault table has `AGE`, `STATUS`, and `TASK` columns, oldest first by
`[created::…]` date then line; rows without a date go last, in file order.
`STATUS` is the task checkbox (`[ ]`, `[x]`, etc.). `TASK` hints mirror Keep:
`☐ n ☑ m` for descendants, plus `↺ still in Keep` for any vault task whose
marker id is a non-archived note in the snapshot (pinned, shared, and empty
notes count too). `↺ still in Keep` never appears with `-s vault`.

Empty states: a missing target file shows a dim
`gkeep_inbox.md not found · create it or set gkeep.target` line; otherwise an
empty vault shows `No open tasks`, or `No tasks` with `--all`. An empty Keep
shows `Keep inbox is empty ✓`.

Explicit selection with `-i` overrides pinned/shared skips. An exact Keep id
always wins; otherwise the value is a prefix match on REF, and zero or
multiple matches (including an empty `-i ""`) exit 2 listing the candidates.
The footer names non-zero state counts (`N new · M pending · K revised`,
skipped states only when non-zero) and the next command to run
(`→ bob gkeep pull`), or the all-clear `✓ Keep inbox is clear` with
`· N pinned stays in Keep` / `· N shared stays in Keep` suffixes as needed.

## Pull

A note is archived only after its current content is verifiably in the vault:
written atomically, fsynced, re-read and parsed, and committed when the vault
is a Git worktree. Notes edited in Keep during a pull stay in Keep; the next
pull writes the revision. Nothing is ever deleted from Keep.

The pipeline: snapshot, ledger/journal scan, plan, URL clip pre-pass,
vault lock, compare-and-swap durable write, parse-verify, scoped Git commit,
content-guarded archive, journal append.
`-d, --dry-run` prints the exact Markdown a pull would write and changes
nothing. `-n, --no-archive` writes and verifies but leaves notes in Keep.
`-C, --no-commit` skips the vault Git commit. `-R, --no-ref` keeps URL-only
notes as inbox tasks instead of clipping them for the reading queue.
`-q, --quiet` prints only errors. `-l, --limit N` takes the first N
actionable notes, oldest first.
A per-host pull lock serializes concurrent pulls (exit 1 when held); dry runs
take no lock.

### URL-only notes go to the reading queue

A note that holds exactly one bare public link and nothing else — an empty
title with a link body, an empty body with a link title, a title equal to
the link, or a page title equal to the shared-link preview title — is
clipped into the reading queue instead of becoming a task. Lists, notes
with attachments, shared notes (even with `-S`), multi-link or
link-plus-prose notes, corporate short links, IP literals, and
`highlights.url_routing.exclude_hosts` entries stay tasks. Pinned notes
included with `-p` follow the rule like any other note.

After planning and before the vault lock, each URL-only note clips
sequentially through the same ingest as `bob ref create`, announced as
`Clipping <display> (i/N)`. The outcomes:

| Outcome | Meaning | Journal | Archive |
| --- | --- | --- | --- |
| `created` | A fresh intake PDF was clipped | `ref_created` at once | Archived |
| `already_in_library` | A ref note already records the link | `ref_created` at once | Archived |
| `already_queued` | An intake PDF is already queued | `ref_created` at once | Archived |
| `failed_retryable` | Network, timeout, browser, or dependency failure | None | Left in Keep; exit 1 |
| `failed_permanent` | Blocked, thin, render, or content failure | None | Written as a task with a ⚠️ child, then archived |

A retryable failure leaves the note in Keep for the next pull and counts
toward `summary.failed`. A permanent failure renders exactly as today with
`⚠️ Clip failed (<kind>): <message> · retry: bob ref create <url>` as a
child just before the `Source:` line. A pull whose notes all clip needs no
`gkeep_inbox.md` and takes no vault lock. `pull -d` never clips: it shows
`would clip → reading queue` rows (with the offline library verdict for
library hits) followed by `N links would be clipped into the reading queue`,
and the Markdown section excludes URL-only notes. `list` marks notes a pull
would clip with a `🔗 ref` hint.

Failure matrix:

| Failure | Outcome |
| --- | --- |
| Lock held by another pull | Exit 1 with the "already running" message |
| Unknown or ambiguous `--id` | Exit 2 listing the candidates |
| Missing target note | Exit 2 |
| No stored token, `uv` missing, bad config | Exit 2 |
| Verify failure | Exit 1; only failing notes stay unarchived; verified notes still archive |
| Commit failure | Exit 1; nothing is archived |
| Missing `git` with no `.git` ancestor | Writes and verifies, `commit: null`, archives |
| A note changed in Keep during the pull | Left in Keep (`changed`); exit 1 |
| A note gone, trashed, or deleted | Reported (`missing`); exit 1 |
| Vault not a Git worktree | Writes and verifies, `commit: null`, archives |

## Rendering and escaping

One Keep note becomes one top-level task. Keep text is data — never markup
and never capture grammar — so everything from the note is normalized and
escaped. Normalization drops `\r`, turns tabs into a space, removes
zero-width characters (U+200B–U+200D, U+FEFF), trims each line, and drops
blank lines. A note whose text is only zero-width characters counts as
`empty`. The task line comes from `capture::format_task_line` with the Keep
**created** date in local time; status is always `[ ]`. The note title comes
first, else the first non-blank text line (inner whitespace collapsed, one
leading `- `/`* `/`• ` bullet stripped, then removed from the children), else
the first list item, else a `Google Keep image note` fallback when attachments
exist, else an `Untitled Google Keep list (N items)` fallback
(`1 item` singular). Children use the target note's indent unit (what
`bob capture` would use); list children nest one level deeper when indented
or checked; OCR text nests under an attachment summary line
(`📎 N image(s)/drawing(s)/audio clip(s)/file(s) stay(s) in Google Keep`,
with unknown kinds as `other` → `file(s)`); the `Source:` child links back
to Keep with the local created time, labels, and the marker (with a
`· revised` flag for revisions). Source URLs percent-encode `%`, `(`, `)`,
`<`, `>`, `[`, `]`, and whitespace, so a crafted id cannot plant a marker.

Escaping: `#task` tokens → `\#task`; trailing ` ^id` block ids (including a
caret starting the text or following Unicode whitespace/NBSP) → `\^id`;
`%%` → `%&#37;`; every colon in runs of two or more (`:::` → `\:\:\:`);
child-leading `#{1,6} ` headings (including a bare `#`–`######` with nothing
after it), `>`, `N.`/`N)`, `|`, `+ `/`- `/`* ` (including a bare `-`/`*`/`+`
child), thematic breaks (`---`/`***`/`___`, spaces allowed), and code fences
(leading ` ``` `/`~~~`) gain a leading backslash.

```markdown
- [ ] #task Call dentist about crown [created::2026-09-27]
	- They close at 5 on Fridays
	- Source: [Google Keep](https://keep.google.com/u/0/#NOTE/…) · 2026-09-27 21:14 %%gkeep:v1:<id>:3f9c2e1d0a7b%%
- [ ] #task Hardware store [created::2026-09-26]
	- [ ] wood screws
		- [ ] #8 × 1¼"
	- [x] sandpaper
	- 📎 1 image stays in Google Keep
		- RECEIPT TOTAL 12.99
	- Source: [Google Keep](…) · 2026-09-26 08:02 · 🏷 errands %%gkeep:v1:<id>:9b1e44c07a2d%%
```

## Marker, ledger, and journal

**Marker.** `%%gkeep:v1:<id>:<fp12>%%`, at the end of the task's `Source:`
child line. Obsidian hides `%%…%%` comments in Live Preview and Reading view.
Ids are percent-encoded outside `[A-Za-z0-9._-]`; the raw id never goes into
a block id. Parse regex: `%%gkeep:v1:([A-Za-z0-9._%-]+):([0-9a-f]{12})%%`.

**Fingerprint.** `KeepContent {title, text, items: [KeepItem {text, checked,
indented}]}` serializes with `serde_json::to_string` in exactly that field
order; `fp` is the first 12 lowercase hex digits of its SHA-256. Attachment
OCR text is excluded, because it can arrive asynchronously and must not make
a note look revised.

**Ledger.** A scan over every `.md` file in the vault, including `done/`,
skipping the always-excluded note directories. A cheap
`contents.contains("%%gkeep:")` pre-filter skips files without markers; each
hit records `{id, fp, path, line}`. Tasks keep their `Source:` child through
triage and `task archive` (formerly `move-done-tasks`, still accepted), so
re-runs are idempotent across hosts with no
local state. More than one ledger entry with the same `(id, fp)` warns,
naming each `path:line`.

**Journal.** `$XDG_STATE_HOME/bob-cli/gkeep/journal.jsonl` (directory `0700`,
file `0600`), append-only and fsynced per batch. Records look like
`{"ts","event":"written"|"ref_created"|"archived"|"archive_refused","id","ref","fp","path","commit","status","url"}`.
It never stores note bodies. A `written` record acts as a lower-priority
ledger entry: the backstop for a task whose marker was deleted during triage
while the note was still in Keep. A `ref_created` record (with the intake
PDF or existing ref note in `path` and the clipped URL in `url`) covers a
clipped link the same way: a re-pull archives it without clipping again. An
older bob reading a newer journal counts `ref_created` lines as corrupt and
warns.

**Archive guard.** Every archive target carries the note's content plus its
live attachment count (`expect_attachments`). The adapter refuses the
archive with `changed` when either differs, so an attachment added mid-pull
keeps the note in Keep.

## Exit status

- `0` — success, including "nothing to pull".
- `1` — runtime failure: auth rejected, network, adapter crash or timeout,
  lock contention, verify or commit failure, or any note that could not be
  archived.
- `2` — usage or setup error: clap usage errors, missing or invalid `gkeep`
  config, unknown or ambiguous `--id`, a missing target note, `uv` not found,
  or no stored token.

Errors print `bob gkeep <cmd>: <message>` to stderr (red `error` prefix on a
TTY) with an optional dim `  hint: …` line, for example
`hint: run \`bob gkeep login\``. In JSON mode, failures print
`{"schema_version":1,"ok":false,"error":{"kind":…,"message":…,"hint":…}}` to
stdout.

## JSON output

Every JSON document carries `schema_version: 1`.

`list -f json`:

```json
{
  "schema_version": 1, "ok": true,
  "keep": {
    "account": "bryanbugyi34@gmail.com", "fetched_at": "2026-09-27T21:14:03Z",
    "notes": [{
      "id": "…", "ref": "3f9c2e1", "kind": "note|list",
      "title": "…", "state": "new|pending|revised|empty|pinned|shared|archived",
      "pinned": false, "shared": false, "archived": false,
      "labels": ["errands"], "created": "2026-09-27T21:14:03Z",
      "edited": "…Z", "url": "https://keep.google.com/…|null",
      "fingerprint": "3f9c2e1d0a7b",
      "lines": 0, "items_open": 0, "items_checked": 0, "attachments": 0
      // only on notes a pull would clip:
      // "clip": {"url": "…", "display": "…", "verdict": "not_found|…"}
    }]
  } | {"error": {"kind": "…", "message": "…", "hint": "…|null"}} | null,
  "vault": {"path": "gkeep_inbox.md", "tasks": [{
    "line": 7, "status": "[ ]", "description": "…",
    "created": "2026-09-27|null",
    "keep_id": "…|null", "keep_state": "new|…|null"
  }]} | null,
  "summary": {"new": 0, "pending": 0, "revised": 0, "skipped": 0, "duplicates": 0}
}
```

`keep` is `null` for `-s vault`; `vault` is `null` for `-s keep`. A Keep
snapshot failure reports `keep: {"error": …}` with `ok: false` and still
prints the vault; config, token, and adapter-resolve failures print the
generic error document instead. A missing target in JSON gives `tasks: []`.

`pull -f json`:

```json
{
  "schema_version": 1, "ok": true, "dry_run": false,
  "archive_enabled": true, "commit_enabled": true,
  "target": "gkeep_inbox.md", "commit": "<40-char-sha>|null",
  "notes": [{
    "id": "…", "ref": "3f9c2e1", "title": "…",
    "state": "new|pending|revised|empty|pinned|shared|archived",
    "action": "write|write_revision|archive_only|create_ref|skip",
    "skip_reason": "empty|pinned|shared|archived|null", "written": true,
    "archive": "archived|already_archived|changed|missing|error|not_requested|not_attempted",
    "detail": "…|null"
    // only on URL-only notes:
    // "clip": {"url": "…", "display": "…",
    //   "outcome": "created|already_in_library|already_queued|failed_retryable|failed_permanent|would_clip",
    //   "pdf": "…", "existing": "…",
    //   "error": {"kind": "…", "message": "…", "retryable": true}}
  }],
  "markdown": "…|null",
  "summary": {"written": 0, "archived": 0, "skipped": 0, "failed": 0,
    "refs": {"clipped": 0, "already_in_library": 0, "already_queued": 0,
      "failed_retryable": 0, "failed_permanent": 0}},
  "error": {"kind": "…", "message": "…", "hint": "…|null"}
}
```

`commit` is the full 40-character SHA, or `null`; never a short SHA.
`error` appears only when the adapter fails during archive. `markdown` is
the inserted block (also on archive failure); it is `null` when nothing was
written. `skip_reason` is `empty|pinned|shared|archived|null`. The `archive`
values mean: `not_requested` for skipped notes and whenever archiving is
disabled with `-n` (including dry runs); `not_attempted` for dry runs with
archiving enabled and whenever archiving was never reached; the rest as
listed. `-q -f json` prints nothing on success.

Dry runs report `written: false` per note and `summary.written: 0` with the
preview in `markdown`. An archive crash reports exactly one document with
`ok: false`, every due note as `archive: "error"` with the adapter message
as `detail`, and the top-level `error` object.

`doctor -f json` reports `{ok, checks[]}` with
`checks[]: {name, status: "ok|warn|fail|skip", summary, hint}` where `hint`
may be `null` and `ok` is false (with exit 1) when any check fails. Check
names are `config|account|token|adapter|keep|target|git`.

## Security

The master token is equivalent to a password: it lives in `pass` (via
`token_command` / `token_store_command`), travels only over stdin to the
adapter, and never appears in argv, logs, errors, or JSON. The adapter state
cache and the journal are `0600`. Revoke access by removing the Android
device in the Google account.

## Known limits

Rich text and reminders are not carried into the vault, and media stays in
Google Keep (an attachment summary line plus OCR text is all that lands in
the task). There is no scheduled runner: draining the inbox is a manual
`bob gkeep pull`, not a `bob nightly` step.

## Environment

`BOB_DIR` sets the Bob vault directory. It defaults to `~/bob`.

`BOB_CONFIG_FILE` sets the exact config file holding the `gkeep:` section.
When unset, Bob uses `$XDG_CONFIG_HOME/bob/config.yml`, then
`~/.config/bob/config.yml`.

`BOB_GKEEP_ADAPTER` is the path of an executable that speaks the adapter
protocol and replaces `uv run --script …`. It is the test hook: tests point
it at a fake adapter and configure everything else through a temporary
config file.

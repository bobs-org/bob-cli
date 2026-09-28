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
bob gkeep pull [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-i|--id REF]... [-p|--include-pinned] [-S|--include-shared] [-l|--limit N] [-n|--no-archive] [-C|--no-commit] [-q|--quiet]
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
`target` must be vault-relative and end in `.md`; `timeout_secs` must be
greater than 0.

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

The Keep table has `REF`, `AGE`, `KIND`, `STATE`, and `NOTE` columns. `REF`
is the first 7 hex digits of `sha256(id)`; it is shown in `list` and accepted
by `pull -i`. States, oldest first:

| State | Meaning |
| --- | --- |
| `new` | Not in the vault; a pull writes it, then archives it |
| `pending` | Already in the vault with the same content; a pull archives only |
| `revised` | In the vault with older content; a pull writes the revision, then archives |
| `empty` | No title, text, items, or attachments; skipped |
| `pinned` | Pinned; skipped unless `--include-pinned` or selected with `-i` |
| `shared` | Shared with collaborators; skipped unless `--include-shared` or selected with `-i` |
| `archived` | Already archived (only with `--all`) |

Explicit selection with `-i` overrides pinned/shared skips. An exact Keep id
always wins; otherwise the value is a prefix match on REF, and zero or
multiple matches exit 2 listing the candidates. The footer names non-zero
state counts and the next command to run.

## Pull

A note is archived only after its current content is verifiably in the vault:
written atomically, fsynced, re-read and parsed, and committed when the vault
is a Git worktree. Notes edited in Keep during a pull stay in Keep; the next
pull writes the revision. Nothing is ever deleted from Keep.

The pipeline: snapshot, vault lock, plan, compare-and-swap durable write,
parse-verify, scoped Git commit, content-guarded archive, journal append.
`-d, --dry-run` prints the exact Markdown a pull would write and changes
nothing. `-n, --no-archive` writes and verifies but leaves notes in Keep.
`-C, --no-commit` skips the vault Git commit. `-q, --quiet` prints only
errors. `-l, --limit N` takes the first N actionable notes, oldest first.
A per-host pull lock serializes concurrent pulls (exit 1 when held); dry runs
take no lock.

Failure matrix:

| Failure | Outcome |
| --- | --- |
| Lock held by another pull | Exit 1 with the "already running" message |
| Unknown or ambiguous `--id` | Exit 2 listing the candidates |
| Missing target note | Exit 2 |
| No stored token, `uv` missing, bad config | Exit 2 |
| Verify or commit failure | Exit 1; nothing is archived |
| A note changed in Keep during the pull | Left in Keep (`changed`); exit 1 |
| A note gone, trashed, or deleted | Reported (`missing`); exit 1 |
| Vault not a Git worktree | Writes and verifies, `commit: null`, archives |

## Rendering and escaping

One Keep note becomes one top-level task. Keep text is data — never markup
and never capture grammar — so everything from the note is normalized and
escaped. The task line comes from `capture::format_task_line` with the Keep
**created** date in local time; status is always `[ ]`. The note title comes
first, else the first non-blank text line (removed from the children), else
the first list item, else an `Untitled Google Keep list (N items)` fallback.
Children use the target note's indent unit (what `bob capture` would use);
list children nest one level deeper when indented or checked; OCR text nests
under an attachment summary line; the `Source:` child links back to Keep with
the edited date, labels, and the marker (with a `· revised` flag for
revisions).

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
triage and `move-done-tasks`, so re-runs are idempotent across hosts with no
local state. More than one ledger entry with the same `(id, fp)` warns,
naming each `path:line`.

**Journal.** `$XDG_STATE_HOME/bob-cli/gkeep/journal.jsonl` (directory `0700`,
file `0600`), append-only and fsynced per batch. Records look like
`{"ts","event":"written"|"archived"|"archive_refused","id","ref","fp","path","commit","status"}`.
It never stores note bodies. A `written` record acts as a lower-priority
ledger entry: the backstop for a task whose marker was deleted during triage
while the note was still in Keep.

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

Every JSON document carries `schema_version: 1`. `list -f json` reports the
planned notes with `id`, `ref`, `state`, and titles plus the vault tasks.
`pull -f json` reports per-note `id`, `ref`, `title`, `state`, `action`,
`written`, `archive` status with `detail`, the `markdown` that was (or would
be) written, the commit SHA (null outside a Git worktree or with
`--no-commit`), and a `written / archived / skipped / failed` summary.
`doctor -f json` reports each checklist row (`config`, `account`, `token`,
`adapter`, `keep`, `target`, `git`) with `ok`, `warn`, `fail`, or `skip`.

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

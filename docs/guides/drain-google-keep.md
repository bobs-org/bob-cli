---
title: "Drain Google Keep into tasks"
description: "Check Keep setup, preview a pull, write tasks, then archive notes after the vault verifies them."
tags:
  - CLI
---

# Drain Google Keep into tasks

This guide drains Google Keep inbox notes into `gkeep_inbox.md` as Obsidian
tasks. A note is archived in Keep only after its current content is verified
in the vault. Keep never deletes the note.

## Before you start

Install the CLI and point it at your vault. Add a `gkeep:` block to
`~/.config/bob/config.yml` (or `$BOB_CONFIG_FILE`):

```yaml
gkeep:
  email: user@example.com
  token_command: pass show gkeep/master_token
  token_store_command: pass insert -m -f gkeep/master_token
```

`email` must be the Google account that owns the notes. The token commands
print and store a master token (`aas_et/…`); they never put the token on
argv. `uv` must be on `PATH` so the adapter can run. Tests and this page use
synthetic names; do not paste a live cookie into a log or ticket.

## 1. Store the master token and check the setup

```bash
bob gkeep login
bob gkeep doctor -b /path/to/vault
```

`login` reads a Google sign-in cookie from a hidden TTY prompt (or stdin when
stdin is not a TTY), exchanges it for a master token, stores that token, then
checks Keep reachability. `doctor` walks config, account, token, adapter,
Keep, the target note, and git. Confirm every check is `ok` or an explained
skip. Exit 2 with a login hint means the stored value is missing or is still
a sign-in cookie.

## 2. Inspect the Keep and vault inboxes

```bash
bob gkeep -b /path/to/vault
```

```text { .no-copy title="Output" }
Keep
REF      AGE  KIND  STATE  NOTE
3f9c2e1  2d   note  new    Buy milk
a1b2c3d  5h   note  pinned Weekly plan

Vault · gkeep_inbox.md
AGE  STATUS  TASK
3d   [ ]     Call the bank

1 new · 1 pinned · → bob gkeep pull
```

Confirm each `STATE`. `new` and `revised` notes are written on pull.
`pending` notes are already in the vault and archive only. `pinned` and
`shared` stay in Keep unless you pass `--include-pinned`, `--include-shared`,
or `-i REF`. `bob gkeep list -s vault` skips the network.

## 3. Preview the Markdown, then write without archiving

```bash
bob gkeep pull -d -b /path/to/vault
```

The dry run prints the exact Markdown a pull would write and changes
nothing. It still snapshots Keep. Confirm the previewed task text.

```bash
bob gkeep pull -n -b /path/to/vault
```

`-n` writes and verifies tasks, then leaves notes in Keep. Open
`gkeep_inbox.md` and confirm the new checkbox lines.

## 4. Pull and archive

```bash
bob gkeep pull -b /path/to/vault
```

Confirm the command exits 0 and `bob gkeep` reports `✓ Keep inbox is clear`
(pinned or shared notes may remain). A Git vault also gets a scoped commit of
the target note plus import receipts. A URL-only Keep note clips into the
reading queue during this pull instead of becoming a task; see
[Save a link to your reading queue](save-link-to-reading-queue.md).

## If it goes wrong

Exit 2 is setup: missing config, no token, unknown `-i`, or a missing target
note. Run `bob gkeep doctor` and `bob gkeep login`. Exit 1 is a runtime
failure (auth, network, adapter, lock, verify, or commit). Notes edited in
Keep during the pull stay in Keep. Retry after the reported step is fixed;
leave any receipts from the failed attempt in place. Pass `-i REF` from
`bob gkeep list` to pull one note.

## Related

- [Bob Gkeep](../gkeep.md)
- [bob gkeep](../reference/cli/gkeep.md)
- [Save a link to your reading queue](save-link-to-reading-queue.md)
- [Run the morning review walk](run-morning-review.md)
